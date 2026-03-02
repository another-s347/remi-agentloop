//! Composable calculator agent — shared logic.
//!
//! This crate contains the **entire agent architecture** from the
//! composable calculator example, extracted so the same Rust code
//! compiles to native, wasm32-wasip2, and wasm32-unknown-unknown
//! without modification.
//!
//! ```text
//! Layer2: ExternalToolAgent (divide)
//!   └── Layer1: ExternalToolAgent (multiply)
//!         └── AgentLoop (add, subtract)
//!               └── OpenAIClient<T: HttpTransport>
//! ```
//!
//! The `build_agent()` function constructs the fully-composed agent
//! given any `OpenAIClient<T>`. The caller provides the transport.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use async_stream::stream;
use futures::{Stream, StreamExt};

use remi_agentloop::prelude::*;
use remi_agentloop::tool_macro as tool;

// ── Tools (pure computation — no I/O, no platform deps) ──────────────────────

/// Add two numbers together.
#[tool]
async fn add(a: f64, b: f64) -> f64 {
    a + b
}

/// Subtract b from a.
#[tool]
async fn subtract(a: f64, b: f64) -> f64 {
    a - b
}

/// Multiply two numbers together.
#[tool]
async fn multiply(a: f64, b: f64) -> f64 {
    a * b
}

/// Divide a by b.
#[tool]
async fn divide(a: f64, b: f64) -> f64 {
    a / b
}

// ── ExternalToolAgent ────────────────────────────────────────────────────────

/// A composable tool-execution layer.
///
/// Wraps any `Agent<Request=LoopInput, Response=AgentEvent>`, adds its own
/// tools via `DefaultToolRegistry`, and handles `NeedToolExecution` for the
/// tools it owns.  Tools it cannot handle are yielded upward.
pub struct ExternalToolAgent<A: Agent> {
    inner: A,
    tools: DefaultToolRegistry,
}

impl<A> ExternalToolAgent<A>
where
    A: Agent<Request = LoopInput, Response = AgentEvent, Error = AgentError>,
{
    pub fn new(inner: A) -> Self {
        Self {
            inner,
            tools: DefaultToolRegistry::new(),
        }
    }

    pub fn tool(mut self, t: impl Tool + Send + Sync + 'static) -> Self {
        self.tools.register(t);
        self
    }
}

impl<A> Agent for ExternalToolAgent<A>
where
    A: Agent<Request = LoopInput, Response = AgentEvent, Error = AgentError>,
{
    type Request = LoopInput;
    type Response = AgentEvent;
    type Error = AgentError;

    fn chat(
        &self,
        input: LoopInput,
    ) -> impl Future<Output = Result<impl Stream<Item = AgentEvent>, AgentError>> {
        async move {
            let first_input = match input {
                LoopInput::Start {
                    content,
                    history,
                    mut extra_tools,
                    model,
                    temperature,
                    max_tokens,
                    metadata,
                } => {
                    let user_state = serde_json::Value::Null;
                    extra_tools.extend(self.tools.definitions(&user_state));
                    LoopInput::Start {
                        content,
                        history,
                        extra_tools,
                        model,
                        temperature,
                        max_tokens,
                        metadata,
                    }
                }
                resume @ LoopInput::Resume { .. } => resume,
            };

            let mut next_input = Some(first_input);

            Ok(stream! {
                loop {
                    let input = next_input.take().unwrap();

                    let inner_stream = match self.inner.chat(input).await {
                        Ok(s) => s,
                        Err(e) => {
                            yield AgentEvent::Error(e.into());
                            return;
                        }
                    };
                    let mut inner_stream = std::pin::pin!(inner_stream);

                    let mut done = false;

                    while let Some(event) = inner_stream.next().await {
                        match event {
                            AgentEvent::NeedToolExecution {
                                state,
                                tool_calls,
                                completed_results,
                            } => {
                                let (mine, external): (Vec<_>, Vec<_>) = tool_calls
                                    .iter()
                                    .cloned()
                                    .partition(|tc| self.tools.contains(&tc.name));

                                let mut all_outcomes = completed_results;
                                if !mine.is_empty() {
                                    let resume_map = HashMap::new();
                                    let tool_ctx = ToolContext {
                                        config: AgentConfig::default(),
                                        thread_id: Some(state.thread_id.clone()),
                                        run_id: state.run_id.clone(),
                                        metadata: None,
                                        user_state: Arc::new(RwLock::new(
                                            state.user_state.clone(),
                                        )),
                                    };

                                    let results = self
                                        .tools
                                        .execute_parallel(&mine, &resume_map, &tool_ctx)
                                        .await;

                                    for (tool_call_id, tool_result) in results {
                                        let tc =
                                            mine.iter().find(|p| p.id == tool_call_id).unwrap();
                                        match tool_result {
                                            Ok(ToolResult::Output(mut ts)) => {
                                                let mut last_result = None;
                                                while let Some(output) = ts.next().await {
                                                    match output {
                                                        ToolOutput::Delta(d) => {
                                                            yield AgentEvent::ToolDelta {
                                                                id: tool_call_id.clone(),
                                                                name: tc.name.clone(),
                                                                delta: d,
                                                            };
                                                        }
                                                        ToolOutput::Result(r) => {
                                                            last_result = Some(r);
                                                        }
                                                    }
                                                }
                                                if let Some(result) = last_result {
                                                    yield AgentEvent::ToolResult {
                                                        id: tool_call_id.clone(),
                                                        name: tc.name.clone(),
                                                        result: result.clone(),
                                                    };
                                                    all_outcomes.push(ToolCallOutcome::Result {
                                                        tool_call_id,
                                                        tool_name: tc.name.clone(),
                                                        result,
                                                    });
                                                }
                                            }
                                            Ok(ToolResult::Interrupt(_)) => {}
                                            Err(e) => {
                                                let msg = e.to_string();
                                                yield AgentEvent::Error(
                                                    AgentError::ToolExecution {
                                                        tool_name: tc.name.clone(),
                                                        message: msg.clone(),
                                                    },
                                                );
                                                all_outcomes.push(ToolCallOutcome::Error {
                                                    tool_call_id,
                                                    tool_name: tc.name.clone(),
                                                    error: msg,
                                                });
                                            }
                                        }
                                    }
                                }

                                if external.is_empty() {
                                    next_input = Some(LoopInput::resume(state, all_outcomes));
                                    break;
                                } else {
                                    yield AgentEvent::NeedToolExecution {
                                        state,
                                        tool_calls: external,
                                        completed_results: all_outcomes,
                                    };
                                    done = true;
                                    break;
                                }
                            }
                            AgentEvent::Done => {
                                done = true;
                                yield AgentEvent::Done;
                                break;
                            }
                            other => yield other,
                        }
                    }

                    if done {
                        return;
                    }
                    if next_input.is_none() {
                        return;
                    }
                }
            })
        }
    }
}

// ── build_agent() — the single entry point ───────────────────────────────────

/// Construct the full 4-layer composable calculator agent.
///
/// Generic over `HttpTransport` — works with:
/// - `ReqwestTransport` (native)
/// - `WitHttpTransport` (wasm32-wasip2, host-injected)
/// - `FetchTransport` (browser wasm)
///
/// The inner agent code is identical across all targets.
pub fn build_agent<T: HttpTransport>(
    oai: OpenAIClient<T>,
) -> ExternalToolAgent<ExternalToolAgent<AgentLoop<OpenAIClient<T>>>> {
    let agent_loop = AgentBuilder::new()
        .model(oai)
        .system(
            "You are a precise calculator. Use the provided tools to compute results. \
             Always use tools, never calculate mentally.",
        )
        .tool(Add::new())
        .tool(Subtract::new())
        .max_turns(10)
        .build_loop();

    let layer1 = ExternalToolAgent::new(agent_loop).tool(Multiply::new());
    ExternalToolAgent::new(layer1).tool(Divide::new())
}
