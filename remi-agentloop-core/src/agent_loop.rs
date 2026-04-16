//! Composable agent loop: step() + tool execution cycle.
//!
//! `AgentLoop` is the core engine that repeatedly calls [`step()`] and
//! executes tools until the model conversation is done or interrupted.
//!
//! It is **pure** — no memory persistence, no run lifecycle.  Those
//! concerns are handled by the outer [`BuiltAgent`](crate::builder::BuiltAgent)
//! layer which observes the [`AgentEvent::Checkpoint`] stream and
//! persists to a [`CheckpointStore`] / [`ContextStore`].
//!
//! ```text
//! chat() / chat_in_thread()
//!   └── run_loop()          ← memory + run lifecycle
//!         └── agent_loop()  ← step + tools + tracing  (this module)
//!               └── step()  ← single model call
//! ```
//!
//! `AgentLoop` implements the [`Agent`] trait, so it can be freely
//! composed with adapters (Logging, Retry, TracingLayer, etc.).

#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

use async_stream::stream;
use futures::{stream::SelectAll, Stream, StreamExt};

use crate::checkpoint::{Checkpoint, CheckpointStatus};
use crate::config::AgentConfig;
use crate::error::AgentError;
use crate::model::ChatModel;
use crate::state::{step, Action, AgentState, StepConfig, StepEvent};
use crate::tool::registry::ToolRegistry;
use crate::tool::{ToolDefinitionContext, ToolOutput, ToolResult};
use crate::tracing::{
    DynTracer, ExternalToolResultTrace, InterruptTrace, ModelEndTrace, ModelStartTrace,
    ResumeTrace, RunEndTrace, RunStartTrace, RunStatus, ToolCallTrace, ToolEndTrace,
    ToolExecutionHandoffTrace, ToolOutcomeTrace, ToolStartTrace, TurnStartTrace,
};
use crate::types::{
    AgentEvent, ChatCtx, Content, InterruptInfo, Message, ParsedToolCall, RunId, SpanKind,
    SpanNode, ThreadId, ToolCallOutcome,
};

// ── AgentLoop ─────────────────────────────────────────────────────────────────

/// Composable step + tool execution loop.
///
/// Yields [`AgentEvent`]s including [`AgentEvent::Checkpoint`] at key
/// lifecycle boundaries (so outer layers can persist and resume).
pub struct AgentLoop<M: ChatModel> {
    pub(crate) model: M,
    pub(crate) tools: Box<dyn ToolRegistry>,
    pub(crate) config: AgentConfig,
    pub(crate) tracer: Option<Box<dyn DynTracer>>,
    pub(crate) system_prompt: String,
    pub(crate) max_turns: usize,
}

#[derive(Default)]
struct ForwardedSubagentTraceState {
    thread_id: Option<ThreadId>,
    run_id: Option<RunId>,
    metadata: Option<serde_json::Value>,
    run_span: Option<SpanNode>,
    run_started_at: Option<Instant>,
    turn: usize,
    model_span: Option<SpanNode>,
    model_started_at: Option<Instant>,
    model_response: String,
    prompt_tokens: u32,
    completion_tokens: u32,
    total_prompt_tokens: u32,
    total_completion_tokens: u32,
    tool_calls: std::collections::HashMap<String, ForwardedToolTrace>,
}

struct ForwardedToolTrace {
    name: String,
    arguments_delta: String,
    started_at: Instant,
    result: Option<String>,
    interrupted: bool,
}

impl<M: ChatModel> AgentLoop<M> {
    fn outcome_trace(outcome: &ToolCallOutcome) -> ToolOutcomeTrace {
        match outcome {
            ToolCallOutcome::Result {
                tool_call_id,
                tool_name,
                content,
            } => ToolOutcomeTrace {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                result: Some(content.text_content()),
                error: None,
            },
            ToolCallOutcome::Error {
                tool_call_id,
                tool_name,
                error,
            } => ToolOutcomeTrace {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                result: None,
                error: Some(error.clone()),
            },
        }
    }

    fn derive_run_span(ctx: &ChatCtx, run_id: &RunId) -> SpanNode {
        let parent = ctx.snapshot().state.span;
        SpanNode::derived(SpanKind::Run, format!("run:{}", run_id.0), parent.as_ref())
    }

    fn derive_model_span(run_span: &SpanNode, turn: usize) -> SpanNode {
        run_span.derived_child(SpanKind::Model, format!("turn:{turn}"))
    }

    fn derive_tool_span(run_span: &SpanNode, tool_call_id: &str, tool_name: &str) -> SpanNode {
        run_span.derived_child(SpanKind::Tool, format!("{tool_name}:{tool_call_id}"))
    }

    fn derive_subagent_span(tool_span: &SpanNode, tool_call_id: &str, tool_name: &str) -> SpanNode {
        tool_span.derived_child(SpanKind::Subagent, format!("{tool_name}:{tool_call_id}"))
    }

    async fn start_forwarded_model_trace(
        tracer: &dyn DynTracer,
        trace: &mut ForwardedSubagentTraceState,
    ) {
        let Some(run_span) = trace.run_span.as_ref() else {
            return;
        };
        if trace.model_span.is_some() {
            return;
        }

        let run_id = trace.run_id.clone().unwrap_or_else(RunId::new);
        let turn = trace.turn.max(1);
        let model_span = Self::derive_model_span(run_span, turn);
        tracer
            .on_model_start(&ModelStartTrace {
                span: model_span.clone(),
                run_id,
                turn,
                call_index: turn.saturating_sub(1),
                model: "subagent".to_string(),
                messages: vec![],
                tools: vec![],
                timestamp: chrono::Utc::now(),
            })
            .await;
        trace.model_span = Some(model_span);
        trace.model_started_at = Some(Instant::now());
        trace.model_response.clear();
        trace.prompt_tokens = 0;
        trace.completion_tokens = 0;
        trace.tool_calls.clear();
    }

    async fn finish_forwarded_model_trace(
        tracer: &dyn DynTracer,
        trace: &mut ForwardedSubagentTraceState,
    ) {
        let Some(model_span) = trace.model_span.take() else {
            return;
        };

        let tool_calls = trace
            .tool_calls
            .iter()
            .map(|(id, tool)| ToolCallTrace {
                id: id.clone(),
                name: tool.name.clone(),
                arguments: serde_json::from_str(&tool.arguments_delta)
                    .unwrap_or(serde_json::Value::Null),
                result: tool.result.clone(),
                interrupted: tool.interrupted,
                duration: tool.started_at.elapsed(),
            })
            .collect();

        tracer
            .on_model_end(&ModelEndTrace {
                span: model_span,
                run_id: trace.run_id.clone().unwrap_or_else(RunId::new),
                turn: trace.turn.max(1),
                call_index: trace.turn.max(1).saturating_sub(1),
                response_text: (!trace.model_response.is_empty()).then(|| trace.model_response.clone()),
                tool_calls,
                prompt_tokens: trace.prompt_tokens,
                completion_tokens: trace.completion_tokens,
                duration: trace
                    .model_started_at
                    .take()
                    .map(|started| started.elapsed())
                    .unwrap_or(std::time::Duration::ZERO),
                timestamp: chrono::Utc::now(),
            })
            .await;

        trace.model_response.clear();
        trace.tool_calls.clear();
    }

    async fn trace_forwarded_subagent_event(
        tracer: &dyn DynTracer,
        trace: &mut ForwardedSubagentTraceState,
        tool_span: &SpanNode,
        tool_call_id: &str,
        tool_name: &str,
        payload: &serde_json::Value,
    ) {
        let Some(event_type) = payload.get("type").and_then(|value| value.as_str()) else {
            return;
        };

        match event_type {
            "run_start" => {
                let run_id = payload
                    .get("run_id")
                    .and_then(|value| value.as_str())
                    .map(|value| RunId(value.to_string()))
                    .unwrap_or_else(RunId::new);
                let thread_id = payload
                    .get("thread_id")
                    .and_then(|value| value.as_str())
                    .map(|value| ThreadId(value.to_string()));
                let run_span = Self::derive_subagent_span(tool_span, tool_call_id, tool_name)
                    .derived_child(SpanKind::Run, format!("run:{}", run_id.0));

                trace.thread_id = thread_id.clone();
                trace.run_id = Some(run_id.clone());
                trace.metadata = payload.get("metadata").cloned().filter(|value| !value.is_null());
                trace.run_span = Some(run_span.clone());
                trace.run_started_at = Some(Instant::now());
                trace.turn = 1;
                trace.model_span = None;
                trace.model_started_at = None;
                trace.model_response.clear();
                trace.prompt_tokens = 0;
                trace.completion_tokens = 0;
                trace.total_prompt_tokens = 0;
                trace.total_completion_tokens = 0;
                trace.tool_calls.clear();

                tracer
                    .on_run_start(&RunStartTrace {
                        span: run_span,
                        thread_id,
                        run_id,
                        model: "subagent".to_string(),
                        system_prompt: None,
                        input_messages: vec![],
                        metadata: trace.metadata.clone(),
                        timestamp: chrono::Utc::now(),
                    })
                    .await;

                Self::start_forwarded_model_trace(tracer, trace).await;
            }
            "text_delta" => {
                if trace.run_span.is_some() {
                    Self::start_forwarded_model_trace(tracer, trace).await;
                    if let Some(content) = payload.get("content").and_then(|value| value.as_str()) {
                        trace.model_response.push_str(content);
                    }
                }
            }
            "tool_call_start" => {
                if trace.run_span.is_some() {
                    Self::start_forwarded_model_trace(tracer, trace).await;
                    let Some(id) = payload.get("id").and_then(|value| value.as_str()) else {
                        return;
                    };
                    let Some(name) = payload.get("name").and_then(|value| value.as_str()) else {
                        return;
                    };

                    trace.tool_calls.insert(
                        id.to_string(),
                        ForwardedToolTrace {
                            name: name.to_string(),
                            arguments_delta: String::new(),
                            started_at: Instant::now(),
                            result: None,
                            interrupted: false,
                        },
                    );

                    tracer
                        .on_tool_start(&ToolStartTrace {
                            span: Self::derive_tool_span(
                                trace.run_span.as_ref().unwrap(),
                                id,
                                name,
                            ),
                            run_id: trace.run_id.clone().unwrap_or_else(RunId::new),
                            turn: trace.turn.max(1),
                            tool_call_id: id.to_string(),
                            tool_name: name.to_string(),
                            arguments: serde_json::Value::Null,
                            timestamp: chrono::Utc::now(),
                        })
                        .await;
                }
            }
            "tool_call_arguments_delta" => {
                if let Some(id) = payload.get("id").and_then(|value| value.as_str()) {
                    if let Some(tool) = trace.tool_calls.get_mut(id) {
                        if let Some(delta) = payload.get("delta").and_then(|value| value.as_str()) {
                            tool.arguments_delta.push_str(delta);
                        }
                    }
                }
            }
            "tool_result" => {
                let Some(id) = payload.get("id").and_then(|value| value.as_str()) else {
                    return;
                };
                if let Some(tool) = trace.tool_calls.get_mut(id) {
                    let result = payload
                        .get("result")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                    tool.result = Some(result.clone());
                    tracer
                        .on_tool_end(&ToolEndTrace {
                            span: Self::derive_tool_span(
                                trace.run_span.as_ref().unwrap(),
                                id,
                                &tool.name,
                            ),
                            run_id: trace.run_id.clone().unwrap_or_else(RunId::new),
                            turn: trace.turn.max(1),
                            tool_call_id: id.to_string(),
                            tool_name: tool.name.clone(),
                            result: Some(result),
                            interrupted: false,
                            error: None,
                            duration: tool.started_at.elapsed(),
                            timestamp: chrono::Utc::now(),
                        })
                        .await;
                }
            }
            "interrupt" => {
                let interrupts: Vec<InterruptInfo> = payload
                    .get("interrupts")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_default();
                if trace.run_span.is_some() {
                    tracer
                        .on_interrupt(&InterruptTrace {
                            span: trace.run_span.as_ref().unwrap().clone(),
                            run_id: trace.run_id.clone().unwrap_or_else(RunId::new),
                            interrupts,
                            timestamp: chrono::Utc::now(),
                        })
                        .await;
                    Self::finish_forwarded_model_trace(tracer, trace).await;
                    tracer
                        .on_run_end(&RunEndTrace {
                            span: trace.run_span.as_ref().unwrap().clone(),
                            run_id: trace.run_id.clone().unwrap_or_else(RunId::new),
                            status: RunStatus::Interrupted,
                            output_messages: vec![],
                            total_turns: trace.turn.max(1),
                            total_prompt_tokens: trace.total_prompt_tokens,
                            total_completion_tokens: trace.total_completion_tokens,
                            duration: trace
                                .run_started_at
                                .take()
                                .map(|started| started.elapsed())
                                .unwrap_or(std::time::Duration::ZERO),
                            error: None,
                            timestamp: chrono::Utc::now(),
                        })
                        .await;
                }
            }
            "usage" => {
                trace.prompt_tokens = payload
                    .get("prompt_tokens")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0) as u32;
                trace.completion_tokens = payload
                    .get("completion_tokens")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0) as u32;
                trace.total_prompt_tokens += trace.prompt_tokens;
                trace.total_completion_tokens += trace.completion_tokens;
                Self::finish_forwarded_model_trace(tracer, trace).await;
            }
            "turn_start" => {
                if trace.run_span.is_none() {
                    return;
                }
                let next_turn = payload
                    .get("turn")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(1) as usize;
                trace.turn = next_turn;
                tracer
                    .on_turn_start(&TurnStartTrace {
                        span: trace.run_span.as_ref().unwrap().clone(),
                        run_id: trace.run_id.clone().unwrap_or_else(RunId::new),
                        turn: trace.turn,
                        timestamp: chrono::Utc::now(),
                    })
                    .await;
                Self::start_forwarded_model_trace(tracer, trace).await;
            }
            "done" | "cancelled" | "error" => {
                if trace.run_span.is_none() {
                    return;
                }
                Self::finish_forwarded_model_trace(tracer, trace).await;
                let status = match event_type {
                    "done" => RunStatus::Completed,
                    "cancelled" => RunStatus::Cancelled,
                    _ => RunStatus::Error,
                };
                let error = payload
                    .get("message")
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string());
                tracer
                    .on_run_end(&RunEndTrace {
                        span: trace.run_span.as_ref().unwrap().clone(),
                        run_id: trace.run_id.clone().unwrap_or_else(RunId::new),
                        status,
                        output_messages: vec![],
                        total_turns: trace.turn.max(1),
                        total_prompt_tokens: trace.total_prompt_tokens,
                        total_completion_tokens: trace.total_completion_tokens,
                        duration: trace
                            .run_started_at
                            .take()
                            .map(|started| started.elapsed())
                            .unwrap_or(std::time::Duration::ZERO),
                        error,
                        timestamp: chrono::Utc::now(),
                    })
                    .await;
            }
            _ => {}
        }
    }

    fn append_partial_response(state: &mut AgentState, partial_response: Option<String>) {
        if let Some(text) = partial_response.filter(|text| !text.is_empty()) {
            state.messages.push(Message {
                id: crate::types::MessageId::new(),
                role: crate::types::Role::Assistant,
                content: Content::text(text),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
                metadata: None,
            });
        }
    }

    fn apply_ctx_to_state(&self, state: &mut AgentState, ctx: &ChatCtx) {
        state.thread_id = ctx.thread_id();
        state.run_id = ctx.run_id();
        let snapshot = ctx.snapshot();
        state.user_state = snapshot.state.user_state.clone();

        if state.config.metadata.is_none() {
            state.config.metadata = snapshot.state.metadata.clone();
        }
    }

    fn tool_definition_context(state: &AgentState) -> ToolDefinitionContext {
        ToolDefinitionContext {
            thread_id: Some(state.thread_id.clone()),
            run_id: Some(state.run_id.clone()),
            metadata: state.config.metadata.clone(),
            user_state: state.user_state.clone(),
        }
    }

    fn refresh_local_tool_definitions(state: &mut AgentState, tools: &dyn ToolRegistry) {
        let ctx = Self::tool_definition_context(state);
        let local_defs = tools.definitions_with_context(&ctx);
        let external_defs: Vec<_> = state
            .tool_definitions
            .iter()
            .filter(|d| !tools.contains(&d.function.name))
            .cloned()
            .collect();
        state.tool_definitions = local_defs;
        state.tool_definitions.extend(external_defs);
    }

    /// Build the initial [`AgentState`] for a new run.
    ///
    /// The returned state has `tool_definitions` populated from the
    /// registry.  If you need to add external tool definitions (for
    /// outer-layer execution), append them to `state.tool_definitions`
    /// before calling [`run()`](Self::run).
    pub fn build_state(&self, messages: Vec<Message>) -> AgentState {
        let model_name = self.config.model.clone().unwrap_or_default();
        let extra_body = match &self.config.extra {
            serde_json::Value::Object(map) => map.clone(),
            _ => serde_json::Map::new(),
        };
        let mut state = AgentState::new(StepConfig {
            model: model_name,
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            metadata: None,
            rate_limit_retry: self.config.rate_limit_retry.clone(),
            extra_body,
        });
        state.system_prompt = if self.system_prompt.is_empty() {
            None
        } else {
            Some(self.system_prompt.clone())
        };
        state.tool_definitions = self
            .tools
            .definitions_with_context(&Self::tool_definition_context(&state));
        state.messages = messages;
        state
    }

    /// Run the step + tool execution loop.
    ///
    /// This is the composable core.  It does **not** persist messages
    /// or manage run lifecycle — it only calls `step()`, executes tools,
    /// and yields events.
    ///
    /// Callers that need persistence should observe
    /// [`AgentEvent::Checkpoint`] in the returned stream.
    pub fn run<'a>(
        &'a self,
        ctx: ChatCtx,
        initial_state: AgentState,
        initial_action: Action,
        emit_run_start: bool,
    ) -> impl Stream<Item = AgentEvent> + 'a {
        let max_turns = self.max_turns;
        let model = &self.model;
        let tools = self.tools.as_ref();
        let tracer = self.tracer.as_deref();

        stream! {
            let run_start_time = Instant::now();
            let mut total_prompt_tokens = 0u32;
            let mut total_completion_tokens = 0u32;

            let mut state = initial_state;
            let mut action = initial_action;
            let mut turn = if state.turn == 0 { 1usize } else { state.turn };
            state.turn = turn;

            // Monotonic checkpoint sequence counter
            #[allow(unused_assignments)]
            let mut checkpoint_seq: u64 = 0;

            // Capture run_id for tracing (stable across turns)
            let run_id = state.run_id.clone();
            let model_name = state.config.model.clone();
            let run_span = Self::derive_run_span(&ctx, &run_id);

            ctx.update(|ctx_state| {
                ctx_state.span = Some(run_span.clone());
            });

            if emit_run_start {
                if let Some(t) = tracer {
                    // Build the full input message list for the trace —
                    // include the upcoming user message so the trace shows
                    // what the model will actually receive.
                    let mut trace_input = state.messages.clone();
                    match &action {
                        Action::UserMessage(message) => trace_input.push(message.clone()),
                        _ => {}
                    }
                    t.on_run_start(&RunStartTrace {
                        span: run_span.clone(),
                        thread_id: Some(state.thread_id.clone()),
                        run_id: run_id.clone(),
                        model: model_name.clone(),
                        system_prompt: state.system_prompt.clone(),
                        input_messages: trace_input,
                        metadata: state.config.metadata.clone(),
                        timestamp: chrono::Utc::now(),
                    }).await;
                }
                yield AgentEvent::RunStart {
                    thread_id: state.thread_id.clone(),
                    run_id: state.run_id.clone(),
                    metadata: state.config.metadata.clone(),
                };
            }

            loop {
                if turn > max_turns {
                    if let Some(t) = tracer {
                        t.on_run_end(&RunEndTrace {
                            span: run_span.clone(),
                            run_id: run_id.clone(),
                            status: RunStatus::MaxTurnsExceeded,
                            output_messages: vec![],
                            total_turns: turn - 1,
                            total_prompt_tokens,
                            total_completion_tokens,
                            duration: run_start_time.elapsed(),
                            error: Some(format!("max turns exceeded: {max_turns}")),
                            timestamp: chrono::Utc::now(),
                        }).await;
                    }
                    yield AgentEvent::Error(AgentError::MaxTurnsExceeded { max: max_turns });
                    return;
                }

                Self::refresh_local_tool_definitions(&mut state, tools);

                // ── Model start trace ─────────────────────────────────
                let tool_names: Vec<String> = state.tool_definitions.iter()
                    .map(|td| td.function.name.clone())
                    .collect();
                let model_call_index = state.model_call_seq;
                state.model_call_seq += 1;
                let model_span = Self::derive_model_span(&run_span, turn);

                if let Some(t) = tracer {
                    t.on_model_start(&ModelStartTrace {
                        span: model_span.clone(),
                        run_id: run_id.clone(),
                        turn,
                        call_index: model_call_index,
                        model: model_name.clone(),
                        messages: state.messages.clone(),
                        tools: tool_names,
                        timestamp: chrono::Utc::now(),
                    }).await;
                }

                let model_start = Instant::now();

                // ── Run one step ──────────────────────────────────────
                let step_stream = step(&ctx, state, action, model);
                let mut step_stream = std::pin::pin!(step_stream);

                let mut next_state: Option<AgentState> = None;
                let mut pending_tool_calls: Option<Vec<ParsedToolCall>> = None;
                let mut cancelled_state: Option<AgentState> = None;
                let mut response_text = String::new();
                let mut step_prompt_tokens = 0u32;
                let mut step_completion_tokens = 0u32;

                while let Some(event) = step_stream.next().await {
                    match event {
                        StepEvent::TextDelta(text) => {
                            response_text.push_str(&text);
                            yield AgentEvent::TextDelta(text);
                        }
                        StepEvent::ReasoningStart => {
                            yield AgentEvent::ThinkingStart;
                        }
                        StepEvent::ReasoningEnd { content } => {
                            yield AgentEvent::ThinkingEnd { content };
                        }
                        StepEvent::ToolCallStart { id, name } => {
                            yield AgentEvent::ToolCallStart { id, name };
                        }
                        StepEvent::ToolCallArgumentsDelta { id, delta } => {
                            yield AgentEvent::ToolCallArgumentsDelta { id, delta };
                        }
                        StepEvent::Usage { prompt_tokens, completion_tokens } => {
                            step_prompt_tokens = prompt_tokens;
                            step_completion_tokens = completion_tokens;
                            total_prompt_tokens += prompt_tokens;
                            total_completion_tokens += completion_tokens;
                            yield AgentEvent::Usage { prompt_tokens, completion_tokens };
                        }
                        StepEvent::Done { state: s } => {
                            next_state = Some(s);
                        }
                        StepEvent::NeedToolExecution { state: s, tool_calls } => {
                            pending_tool_calls = Some(tool_calls);
                            next_state = Some(s);
                        }
                        StepEvent::Cancelled { state: s } => {
                            cancelled_state = Some(s);
                        }
                        StepEvent::Error { state: _, error } => {
                            if let Some(t) = tracer {
                                t.on_model_end(&ModelEndTrace {
                                    span: model_span.clone(),
                                    run_id: run_id.clone(),
                                    turn,
                                    call_index: model_call_index,
                                    response_text: None,
                                    tool_calls: vec![],
                                    prompt_tokens: step_prompt_tokens,
                                    completion_tokens: step_completion_tokens,
                                    duration: model_start.elapsed(),
                                    timestamp: chrono::Utc::now(),
                                }).await;
                                t.on_run_end(&RunEndTrace {
                                    span: run_span.clone(),
                                    run_id: run_id.clone(),
                                    status: RunStatus::Error,
                                    output_messages: vec![],
                                    total_turns: turn,
                                    total_prompt_tokens,
                                    total_completion_tokens,
                                    duration: run_start_time.elapsed(),
                                    error: Some(error.to_string()),
                                    timestamp: chrono::Utc::now(),
                                }).await;
                            }
                            yield AgentEvent::Error(error);
                            return;
                        }
                    }
                }

                if let Some(mut cancelled) = cancelled_state {
                    Self::append_partial_response(
                        &mut cancelled,
                        Some(response_text.clone()).filter(|text| !text.is_empty()),
                    );

                    if let Some(t) = tracer {
                        t.on_model_end(&ModelEndTrace {
                            span: model_span.clone(),
                            run_id: run_id.clone(),
                            turn,
                            call_index: model_call_index,
                            response_text: if response_text.is_empty() { None } else { Some(response_text.clone()) },
                            tool_calls: vec![],
                            prompt_tokens: step_prompt_tokens,
                            completion_tokens: step_completion_tokens,
                            duration: model_start.elapsed(),
                            timestamp: chrono::Utc::now(),
                        }).await;
                        t.on_run_end(&RunEndTrace {
                            span: run_span.clone(),
                            run_id: run_id.clone(),
                            status: RunStatus::Cancelled,
                            output_messages: cancelled.messages.clone(),
                            total_turns: turn,
                            total_prompt_tokens,
                            total_completion_tokens,
                            duration: run_start_time.elapsed(),
                            error: None,
                            timestamp: chrono::Utc::now(),
                        }).await;
                    }

                    yield AgentEvent::Checkpoint(Checkpoint::new(
                        cancelled.thread_id.clone(),
                        run_id.clone(),
                        cancelled,
                        None,
                        turn,
                        CheckpointStatus::Cancelled,
                        checkpoint_seq,
                    ));
                    yield AgentEvent::Cancelled;
                    return;
                }

                state = match next_state {
                    Some(s) => s,
                    None => {
                        yield AgentEvent::Error(AgentError::other("step() ended without terminal event"));
                        return;
                    }
                };

                // ── Model end trace ───────────────────────────────────
                let model_duration = model_start.elapsed();
                let tc_traces: Vec<ToolCallTrace> = pending_tool_calls.as_ref()
                    .map(|tcs| tcs.iter().map(|tc| ToolCallTrace {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                        result: None,
                        interrupted: false,
                        duration: std::time::Duration::ZERO,
                    }).collect())
                    .unwrap_or_default();

                if let Some(t) = tracer {
                    t.on_model_end(&ModelEndTrace {
                        span: model_span.clone(),
                        run_id: run_id.clone(),
                        turn,
                        call_index: model_call_index,
                        response_text: if response_text.is_empty() { None } else { Some(response_text.clone()) },
                        tool_calls: tc_traces,
                        prompt_tokens: step_prompt_tokens,
                        completion_tokens: step_completion_tokens,
                        duration: model_duration,
                        timestamp: chrono::Utc::now(),
                    }).await;
                }

                match pending_tool_calls {
                    None => {
                        // ── Done — no tool calls ──────────────────────
                        if let Some(t) = tracer {
                            t.on_run_end(&RunEndTrace {
                                span: run_span.clone(),
                                run_id: run_id.clone(),
                                status: RunStatus::Completed,
                                output_messages: state.messages.clone(),
                                total_turns: turn,
                                total_prompt_tokens,
                                total_completion_tokens,
                                duration: run_start_time.elapsed(),
                                error: None,
                                timestamp: chrono::Utc::now(),
                            }).await;
                        }
                        // ── Checkpoint: RunDone ───────────────────────
                        yield AgentEvent::Checkpoint(Checkpoint::new(
                            state.thread_id.clone(),
                            run_id.clone(),
                            state.clone(),
                            None,
                            turn,
                            CheckpointStatus::RunDone,
                            checkpoint_seq,
                        ));
                        yield AgentEvent::Done;
                        return;
                    }
                    Some(tool_calls) => {
                        // ── Split: local vs external tool calls ───────
                        let (local_calls, external_calls): (Vec<_>, Vec<_>) =
                            tool_calls.iter().cloned().partition(|tc| tools.contains(&tc.name));

                        // ── Execute local tools ───────────────────────
                        let resume_map = std::collections::HashMap::new();

                        if let Some(t) = tracer {
                            for tc in &local_calls {
                                t.on_tool_start(&ToolStartTrace {
                                    span: Self::derive_tool_span(&run_span, &tc.id, &tc.name),
                                    run_id: run_id.clone(),
                                    turn,
                                    tool_call_id: tc.id.clone(),
                                    tool_name: tc.name.clone(),
                                    arguments: tc.arguments.clone(),
                                    timestamp: chrono::Utc::now(),
                                }).await;
                            }
                        }

                        let tool_exec_start = Instant::now();

                        let mut outcomes: Vec<ToolCallOutcome> = Vec::new();
                        let mut pending_interrupts: Vec<InterruptInfo> = Vec::new();

                        if !local_calls.is_empty() {
                            ctx.update(|ctx_state| {
                                ctx_state.metadata = state.config.metadata.clone();
                                ctx_state.user_state = state.user_state.clone();
                            });

                            let tool_results = tools.execute_parallel(&local_calls, &resume_map, &ctx).await;

                            let mut live_streams: SelectAll<std::pin::Pin<Box<dyn Stream<Item = (String, String, ToolOutput)> + '_>>> = SelectAll::new();
                            let mut final_results: std::collections::HashMap<String, Content> = std::collections::HashMap::new();
                            let mut subagent_traces: std::collections::HashMap<String, ForwardedSubagentTraceState> = std::collections::HashMap::new();

                            for (tool_call_id, tool_result) in tool_results {
                                let tc = local_calls.iter().find(|p| p.id == tool_call_id).unwrap();

                                match tool_result {
                                    Err(e) => {
                                        let msg = e.to_string();
                                        if let Some(t) = tracer {
                                            t.on_tool_end(&ToolEndTrace {
                                                span: Self::derive_tool_span(&run_span, &tool_call_id, &tc.name),
                                                run_id: run_id.clone(),
                                                turn,
                                                tool_call_id: tool_call_id.clone(),
                                                tool_name: tc.name.clone(),
                                                result: Some(msg.clone()),
                                                interrupted: false,
                                                error: Some(msg.clone()),
                                                duration: tool_exec_start.elapsed(),
                                                timestamp: chrono::Utc::now(),
                                            }).await;
                                        }
                                        yield AgentEvent::Error(e);
                                        outcomes.push(ToolCallOutcome::Error {
                                            tool_call_id,
                                            tool_name: tc.name.clone(),
                                            error: msg,
                                        });
                                    }
                                    Ok(ToolResult::Interrupt(req)) => {
                                        if let Some(t) = tracer {
                                            t.on_tool_end(&ToolEndTrace {
                                                span: Self::derive_tool_span(&run_span, &tool_call_id, &tc.name),
                                                run_id: run_id.clone(),
                                                turn,
                                                tool_call_id: tool_call_id.clone(),
                                                tool_name: tc.name.clone(),
                                                result: None,
                                                interrupted: true,
                                                error: None,
                                                duration: tool_exec_start.elapsed(),
                                                timestamp: chrono::Utc::now(),
                                            }).await;
                                        }
                                        pending_interrupts.push(InterruptInfo {
                                            interrupt_id: req.interrupt_id,
                                            tool_call_id: tool_call_id.clone(),
                                            tool_name: tc.name.clone(),
                                            kind: req.kind,
                                            data: req.data,
                                        });
                                    }
                                    Ok(ToolResult::Output(tool_stream)) => {
                                        let stream_call_id = tool_call_id.clone();
                                        let stream_tool_name = tc.name.clone();
                                        live_streams.push(Box::pin(tool_stream.map(move |output| {
                                            (stream_call_id.clone(), stream_tool_name.clone(), output)
                                        })));
                                    }
                                }
                            }

                            while let Some((tool_call_id, tool_name, output)) = live_streams.next().await {
                                if ctx.is_cancelled() {
                                    break;
                                }
                                match output {
                                    ToolOutput::Delta(delta) => {
                                        yield AgentEvent::ToolDelta {
                                            id: tool_call_id,
                                            name: tool_name,
                                            delta,
                                        };
                                    }
                                    ToolOutput::SubSession(mut event) => {
                                        if event.parent_tool_call_id.is_empty() {
                                            event.parent_tool_call_id = tool_call_id.clone();
                                        }
                                        yield AgentEvent::SubSession(event);
                                    }
                                    ToolOutput::Custom { event_type, extra } => {
                                        let wrapped_extra = serde_json::json!({
                                            "tool_call_id": tool_call_id,
                                            "tool_name": tool_name,
                                            "payload": extra,
                                        });

                                        if let Some(t) = tracer {
                                            if event_type == "subagent_event" {
                                                let trace = subagent_traces
                                                    .entry(tool_call_id.clone())
                                                    .or_default();
                                                let tool_span = Self::derive_tool_span(&run_span, &tool_call_id, &tool_name);
                                                Self::trace_forwarded_subagent_event(
                                                    t,
                                                    trace,
                                                    &tool_span,
                                                    &tool_call_id,
                                                    &tool_name,
                                                    &wrapped_extra["payload"],
                                                ).await;
                                            }
                                        }

                                        yield AgentEvent::Custom {
                                            event_type,
                                            extra: wrapped_extra,
                                        };
                                    }
                                    ToolOutput::Result(content) => {
                                        yield AgentEvent::ToolResult {
                                            id: tool_call_id.clone(),
                                            name: tool_name,
                                            result: content.text_content(),
                                        };
                                        final_results.insert(tool_call_id, content);
                                    }
                                }
                            }

                            if !ctx.is_cancelled() {
                                for tc in &local_calls {
                                    if let Some(content) = final_results.remove(&tc.id) {
                                        if let Some(t) = tracer {
                                            t.on_tool_end(&ToolEndTrace {
                                                span: Self::derive_tool_span(&run_span, &tc.id, &tc.name),
                                                run_id: run_id.clone(),
                                                turn,
                                                tool_call_id: tc.id.clone(),
                                                tool_name: tc.name.clone(),
                                                result: Some(content.text_content()),
                                                interrupted: false,
                                                error: None,
                                                duration: tool_exec_start.elapsed(),
                                                timestamp: chrono::Utc::now(),
                                            }).await;
                                        }
                                        outcomes.push(ToolCallOutcome::Result {
                                            tool_call_id: tc.id.clone(),
                                            tool_name: tc.name.clone(),
                                            content,
                                        });
                                    }
                                }
                            }

                            state.user_state = ctx.user_state();
                        }

                        if ctx.is_cancelled() {
                            if let Some(t) = tracer {
                                t.on_run_end(&RunEndTrace {
                                    span: run_span.clone(),
                                    run_id: run_id.clone(),
                                    status: RunStatus::Cancelled,
                                    output_messages: state.messages.clone(),
                                    total_turns: turn,
                                    total_prompt_tokens,
                                    total_completion_tokens,
                                    duration: run_start_time.elapsed(),
                                    error: None,
                                    timestamp: chrono::Utc::now(),
                                }).await;
                            }
                            yield AgentEvent::Checkpoint(Checkpoint::new(
                                state.thread_id.clone(),
                                run_id.clone(),
                                state.clone(),
                                None,
                                turn,
                                CheckpointStatus::Cancelled,
                                checkpoint_seq,
                            ));
                            yield AgentEvent::Cancelled;
                            return;
                        }

                        // ── Handle interrupts ─────────────────────────
                        if !pending_interrupts.is_empty() {
                            if let Some(t) = tracer {
                                t.on_interrupt(&InterruptTrace {
                                    span: run_span.clone(),
                                    run_id: run_id.clone(),
                                    interrupts: pending_interrupts.clone(),
                                    timestamp: chrono::Utc::now(),
                                }).await;
                                t.on_run_end(&RunEndTrace {
                                    span: run_span.clone(),
                                    run_id: run_id.clone(),
                                    status: RunStatus::Interrupted,
                                    output_messages: state.messages.clone(),
                                    total_turns: turn,
                                    total_prompt_tokens,
                                    total_completion_tokens,
                                    duration: run_start_time.elapsed(),
                                    error: None,
                                    timestamp: chrono::Utc::now(),
                                }).await;
                            }
                            // ── Checkpoint: Interrupted ───────────────
                            yield AgentEvent::Checkpoint(Checkpoint::new(
                                state.thread_id.clone(),
                                run_id.clone(),
                                state.clone(),
                                None, // resume via ChatInput::Resume, not via Action
                                turn,
                                CheckpointStatus::Interrupted,
                                checkpoint_seq,
                            ));
                            yield AgentEvent::Interrupt { interrupts: pending_interrupts };
                            return;
                        }

                        // ── External tool calls: yield outward ────────
                        if !external_calls.is_empty() {
                            if let Some(t) = tracer {
                                t.on_tool_execution_handoff(&ToolExecutionHandoffTrace {
                                    run_id: run_id.clone(),
                                    turn,
                                    tool_calls: external_calls.iter().map(|tc| ToolCallTrace {
                                        id: tc.id.clone(),
                                        name: tc.name.clone(),
                                        arguments: tc.arguments.clone(),
                                        result: None,
                                        interrupted: false,
                                        duration: std::time::Duration::ZERO,
                                    }).collect(),
                                    completed_results: outcomes.iter().map(Self::outcome_trace).collect(),
                                    timestamp: chrono::Utc::now(),
                                }).await;
                            }
                            // Write back user_state before yielding
                            state.user_state = ctx.user_state();
                            // ── Checkpoint: AwaitingToolExecution ──────
                            yield AgentEvent::Checkpoint(Checkpoint::new(
                                state.thread_id.clone(),
                                run_id.clone(),
                                state.clone(),
                                None, // resume via LoopInput::Resume externally
                                turn,
                                CheckpointStatus::AwaitingToolExecution,
                                checkpoint_seq,
                            ));
                            yield AgentEvent::NeedToolExecution {
                                state,
                                tool_calls: external_calls,
                                completed_results: outcomes,
                            };
                            return;
                        }

                        // ── All tools local, continue loop ────────────
                        state.user_state = ctx.user_state();

                        // Refresh local tool defs while preserving
                        // externally-injected definitions (those not in
                        // this registry).  This allows outer layers to
                        // inject additional tool defs that survive across turns.
                        Self::refresh_local_tool_definitions(&mut state, tools);

                        // ── Checkpoint: ToolsExecuted ─────────────────
                        let next_action = Action::ToolResults(outcomes);
                        yield AgentEvent::Checkpoint(Checkpoint::new(
                            state.thread_id.clone(),
                            run_id.clone(),
                            state.clone(),
                            Some(next_action.clone()),
                            turn,
                            CheckpointStatus::ToolsExecuted,
                            checkpoint_seq,
                        ));
                        checkpoint_seq += 1;

                        turn += 1;
                        state.turn = turn;
                        if let Some(t) = tracer {
                            t.on_turn_start(&TurnStartTrace {
                                span: run_span.clone(),
                                run_id: run_id.clone(),
                                turn,
                                timestamp: chrono::Utc::now(),
                            }).await;
                        }
                        yield AgentEvent::TurnStart { turn };
                        action = next_action;
                    }
                }
            }
        }
    }
}

// ── Agent impl ────────────────────────────────────────────────────────────────

impl<M: ChatModel> crate::agent::Agent for AgentLoop<M> {
    type Request = crate::types::LoopInput;
    type Response = AgentEvent;
    type Error = AgentError;

    fn chat(
        &self,
        ctx: ChatCtx,
        input: crate::types::LoopInput,
    ) -> impl std::future::Future<Output = Result<impl Stream<Item = AgentEvent>, AgentError>> {
        async move {
            let stream: std::pin::Pin<Box<dyn Stream<Item = AgentEvent> + '_>> = match input {
                crate::types::LoopInput::Start {
                    message,
                    history,
                    extra_tools,
                    model,
                    temperature,
                    max_tokens,
                    metadata,
                } => {
                    let mut state = self.build_state(history);
                    self.apply_ctx_to_state(&mut state, &ctx);

                    if !extra_tools.is_empty() {
                        state.tool_definitions.extend(extra_tools);
                    }
                    if let Some(m) = model {
                        state.config.model = m;
                    }
                    if let Some(t) = temperature {
                        state.config.temperature = Some(t);
                    }
                    if let Some(n) = max_tokens {
                        state.config.max_tokens = Some(n);
                    }
                    if let Some(v) = metadata {
                        state.config.metadata = Some(v);
                    }

                    if ctx.is_cancelled() {
                        state.phase = crate::state::AgentPhase::Done;
                        Box::pin(self.cancel_run(state))
                    } else {
                        let action = Action::UserMessage(message);
                        Box::pin(self.run(ctx.clone(), state, action, true))
                    }
                }
                crate::types::LoopInput::Resume {
                    mut state,
                    pending_interrupts: _,
                    results,
                } => {
                    self.apply_ctx_to_state(&mut state, &ctx);

                    if ctx.is_cancelled() {
                        state.phase = crate::state::AgentPhase::Done;
                        Box::pin(self.cancel_run(state))
                    } else {
                        let outcomes = results.iter().map(Self::outcome_trace).collect::<Vec<_>>();
                        if let Some(t) = self.tracer.as_deref() {
                            t.on_resume(&ResumeTrace {
                                span: Self::derive_run_span(&ctx, &state.run_id),
                                run_id: state.run_id.clone(),
                                payloads_count: results.len(),
                                outcomes: outcomes.clone(),
                                timestamp: chrono::Utc::now(),
                            })
                            .await;

                            for outcome in &outcomes {
                                t.on_external_tool_result(&ExternalToolResultTrace {
                                    run_id: state.run_id.clone(),
                                    tool_call_id: outcome.tool_call_id.clone(),
                                    tool_name: outcome.tool_name.clone(),
                                    result: outcome.result.clone(),
                                    error: outcome.error.clone(),
                                    timestamp: chrono::Utc::now(),
                                }).await;
                            }
                        }
                        Box::pin(self.run(ctx.clone(), state, Action::ToolResults(results), false))
                    }
                }
            };

            Ok(stream)
        }
    }
}

impl<M: ChatModel> AgentLoop<M> {
    /// Produce a `Cancelled` checkpoint and event for the given state.
    ///
    /// This is a lightweight path that does not call `step()`.
    pub fn cancel_run(&self, state: AgentState) -> impl Stream<Item = AgentEvent> + '_ {
        stream! {
            yield AgentEvent::Checkpoint(Checkpoint::new(
                state.thread_id.clone(),
                state.run_id.clone(),
                state,
                None,
                0,
                CheckpointStatus::Cancelled,
                0,
            ));
            yield AgentEvent::Cancelled;
        }
    }

    /// Flush any buffered tracing I/O (e.g. pending LangSmith HTTP calls).
    ///
    /// Call this after the agent event stream has been fully consumed and
    /// before the async runtime shuts down.
    pub async fn flush_tracer(&self) {
        if let Some(t) = self.tracer.as_deref() {
            t.on_flush().await;
        }
    }
}
