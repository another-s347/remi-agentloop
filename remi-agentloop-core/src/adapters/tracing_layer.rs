use async_stream::stream;
use futures::{Stream, StreamExt};
use std::future::Future;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

use crate::agent::{Agent, Layer};
use crate::error::AgentError;
use crate::tracing::{ModelEndTrace, RunEndTrace, RunStartTrace, RunStatus, TurnStartTrace};
use crate::types::AgentEvent;

// ── TracingLayer ──────────────────────────────────────────────────────────────

/// A [`Layer`] that wraps any `Agent<Response = AgentEvent>` with tracing.
///
/// This is useful for adding tracing to agents that don't use `BuiltAgent`
/// (e.g., `ProtocolAgent`, `HttpSseClient`, `WasmAgent`) or for adding
/// an additional tracer at the transport boundary.
///
/// For `BuiltAgent`, prefer using `.tracer()` on the builder, which
/// provides richer per-tool-call tracing.
///
/// ```ignore
/// use remi_agentloop::prelude::*;
/// use remi_agentloop::adapters::tracing_layer::TracingLayer;
/// use remi_agentloop::tracing::stdout::StdoutTracer;
///
/// let agent = some_agent.layer(TracingLayer::new(StdoutTracer));
/// ```
pub struct TracingLayer<T> {
    tracer: T,
}

impl<T> TracingLayer<T> {
    pub fn new(tracer: T) -> Self {
        Self { tracer }
    }
}

impl<A, T> Layer<A> for TracingLayer<T>
where
    A: Agent<Request = crate::types::LoopInput, Response = AgentEvent, Error = AgentError>,
    T: crate::tracing::Tracer + Send + Sync + 'static,
{
    type Output = TracedAgent<A, T>;

    fn layer(self, inner: A) -> Self::Output {
        TracedAgent {
            inner,
            tracer: self.tracer,
        }
    }
}

// ── TracedAgent ───────────────────────────────────────────────────────────────

/// An agent wrapper that emits tracer events by observing the `AgentEvent` stream.
///
/// Created by [`TracingLayer`].
pub struct TracedAgent<A, T> {
    inner: A,
    tracer: T,
}

impl<A, T> Agent for TracedAgent<A, T>
where
    A: Agent<Request = crate::types::LoopInput, Response = AgentEvent, Error = AgentError>,
    T: crate::tracing::Tracer + Send + Sync,
{
    type Request = crate::types::LoopInput;
    type Response = AgentEvent;
    type Error = AgentError;

    fn chat(
        &self,
        req: crate::types::LoopInput,
    ) -> impl Future<Output = Result<impl Stream<Item = AgentEvent>, AgentError>> {
        async move {
            let inner_stream = self.inner.chat(req).await?;

            Ok(stream! {
                let mut inner_stream = std::pin::pin!(inner_stream);

                let run_start_time = Instant::now();
                let mut run_id = crate::types::RunId::new();
                let mut turn = 1usize;
                let mut total_prompt_tokens = 0u32;
                let mut total_completion_tokens = 0u32;

                while let Some(event) = inner_stream.next().await {
                    match &event {
                        AgentEvent::RunStart { run_id: rid, .. } => {
                            run_id = rid.clone();
                            self.tracer.on_run_start(&RunStartTrace {
                                thread_id: None,
                                run_id: run_id.clone(),
                                model: String::new(),
                                system_prompt: None,
                                input_messages: vec![],
                                metadata: None,
                                timestamp: chrono::Utc::now(),
                            }).await;
                        }
                        AgentEvent::TurnStart { turn: t } => {
                            turn = *t;
                            self.tracer.on_turn_start(&TurnStartTrace {
                                run_id: run_id.clone(),
                                turn,
                                timestamp: chrono::Utc::now(),
                            }).await;
                        }
                        AgentEvent::Usage { prompt_tokens, completion_tokens } => {
                            total_prompt_tokens += prompt_tokens;
                            total_completion_tokens += completion_tokens;
                            // ModelEnd fires on Usage
                            self.tracer.on_model_end(&ModelEndTrace {
                                run_id: run_id.clone(),
                                turn,
                                response_text: None,
                                tool_calls: vec![],
                                prompt_tokens: *prompt_tokens,
                                completion_tokens: *completion_tokens,
                                duration: run_start_time.elapsed(),
                                timestamp: chrono::Utc::now(),
                            }).await;
                        }
                        AgentEvent::Done => {
                            self.tracer.on_run_end(&RunEndTrace {
                                run_id: run_id.clone(),
                                status: RunStatus::Completed,
                                output_messages: vec![],
                                total_turns: turn,
                                total_prompt_tokens,
                                total_completion_tokens,
                                duration: run_start_time.elapsed(),
                                error: None,
                                timestamp: chrono::Utc::now(),
                            }).await;
                        }
                        AgentEvent::Error(e) => {
                            self.tracer.on_run_end(&RunEndTrace {
                                run_id: run_id.clone(),
                                status: RunStatus::Error,
                                output_messages: vec![],
                                total_turns: turn,
                                total_prompt_tokens,
                                total_completion_tokens,
                                duration: run_start_time.elapsed(),
                                error: Some(e.to_string()),
                                timestamp: chrono::Utc::now(),
                            }).await;
                        }
                        _ => {}
                    }
                    yield event;
                }
            })
        }
    }
}
