//! Composable calculator: four-layer architecture with multi-turn memory.
//!
//! Demonstrates deeply nested composable agent design using **only** the
//! `Agent` trait — no extra traits needed:
//!
//! ```text
//! App (multi-turn loop — collects NewMessages, manages history)
//!   └── Layer2: ExternalToolAgent (divide)       ← impl Agent
//!         └── Layer1: ExternalToolAgent (multiply) ← impl Agent
//!               └── AgentLoop (add, subtract)      ← impl Agent
//!                     └── step() → model call
//! ```
//!
//! Each `ExternalToolAgent` layer:
//! - Wraps any `A: Agent<Request=LoopInput, Response=AgentEvent>` — the
//!   same `Agent` trait used everywhere in the library
//! - Uses `DefaultToolRegistry` + `#[tool]` macro (same as AgentLoop)
//! - Injects its tool definitions via `LoopInput::extra_tools`
//! - Intercepts `NeedToolExecution`, executes its own tools, resumes via
//!   `LoopInput::Resume` — passes unhandled tools upward
//!
//! Multi-turn memory: the app collects `NewMessages` events and passes
//! conversation history to the next turn via `LoopInput::history()`.
//!
//! Run with:
//!   REMI_API_KEY=... REMI_BASE_URL=... REMI_MODEL=... \
//!     cargo run --example composable_calculator --features http-client

use std::future::Future;
use std::sync::{Arc, RwLock};
use async_stream::stream;
use futures::{Stream, StreamExt};
use remi_agentloop::prelude::*;
use remi_agentloop::tool_macro as tool;

// ── Inner tools (auto-executed by AgentLoop) ─────────────────────────────────

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

// ── Layer 1 tool ─────────────────────────────────────────────────────────────

/// Multiply two numbers together.
#[tool]
async fn multiply(a: f64, b: f64) -> f64 {
    a * b
}

// ── Layer 2 tool ─────────────────────────────────────────────────────────────

/// Divide a by b.
#[tool]
async fn divide(a: f64, b: f64) -> f64 {
    a / b
}

// ── ExternalToolAgent: wraps any Agent, adds tools ───────────────────────────

/// A composable tool-execution layer.
///
/// Wraps any `Agent<Request=LoopInput, Response=AgentEvent>`, adds its own
/// tools via `DefaultToolRegistry`, and handles `NeedToolExecution` for the
/// tools it owns.  Tools it cannot handle are yielded upward.
///
/// This is the *only* trait involved — `Agent`.
struct ExternalToolAgent<A: Agent> {
    inner: A,
    tools: DefaultToolRegistry,
}

impl<A> ExternalToolAgent<A>
where
    A: Agent<Request = LoopInput, Response = AgentEvent, Error = AgentError>,
{
    fn new(inner: A) -> Self {
        Self {
            inner,
            tools: DefaultToolRegistry::new(),
        }
    }

    fn tool(mut self, t: impl Tool + Send + Sync + 'static) -> Self {
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
            // Prepare the input for the inner agent
            let first_input = match input {
                LoopInput::Start { content, history, mut extra_tools, model, temperature, max_tokens, metadata } => {
                    // Inject our tool definitions and pass down
                    let user_state = serde_json::Value::Null;
                    extra_tools.extend(self.tools.definitions(&user_state));
                    LoopInput::Start { content, history, extra_tools, model, temperature, max_tokens, metadata }
                }
                // Resume passes straight through — state belongs to innermost
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
                                // Partition: ours vs. still-external
                                let (mine, external): (Vec<_>, Vec<_>) = tool_calls
                                    .iter()
                                    .cloned()
                                    .partition(|tc| self.tools.contains(&tc.name));

                                // Execute our tools
                                let mut all_outcomes = completed_results;
                                if !mine.is_empty() {
                                    let resume_map = std::collections::HashMap::new();
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
                                        let tc = mine.iter().find(|p| p.id == tool_call_id).unwrap();
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
                                                yield AgentEvent::Error(AgentError::ToolExecution {
                                                    tool_name: tc.name.clone(),
                                                    message: msg.clone(),
                                                });
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
                                    // All handled — resume inner via Agent::chat
                                    next_input = Some(LoopInput::resume(state, all_outcomes));
                                    break; // re-enter loop
                                } else {
                                    // Pass unhandled tools upward
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

                    if done { return; }
                    if next_input.is_none() { return; }
                }
            })
        }
    }
}

// ── App (outermost — multi-turn conversation loop) ───────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("REMI_API_KEY"))
        .expect("OPENAI_API_KEY or REMI_API_KEY must be set");

    let model_name = std::env::var("REMI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());
    let base_url = std::env::var("REMI_BASE_URL")
        .or_else(|_| std::env::var("OPENAI_BASE_URL"))
        .ok();

    let mut oai = OpenAIClient::new(api_key).with_model(model_name.clone());
    if let Some(url) = base_url {
        oai = oai.with_base_url(url);
    }

    // ── Build the 4-layer agent ───────────────────────────────────────────
    //
    //   AgentLoop (add, subtract)  ← impl Agent
    //     → ExternalToolAgent (multiply)  ← impl Agent
    //       → ExternalToolAgent (divide)  ← impl Agent
    //         → App (stream consumer)
    //
    let agent_loop = AgentBuilder::new()
        .model(oai)
        .system("You are a precise calculator. Use the provided tools to compute results. Always use tools, never calculate mentally.")
        .tool(Add::new())
        .tool(Subtract::new())
        .max_turns(10)
        .build_loop();

    let layer1 = ExternalToolAgent::new(agent_loop).tool(Multiply::new());
    let agent = ExternalToolAgent::new(layer1).tool(Divide::new());

    eprintln!("═══ Composable Calculator (4 layers × multi-turn) ═══");
    eprintln!("  AgentLoop:  add, subtract");
    eprintln!("  Layer 1:    multiply");
    eprintln!("  Layer 2:    divide");
    eprintln!("  App:        multi-turn conversation");
    eprintln!();

    // ── Multi-turn conversation ───────────────────────────────────────────
    let questions = [
        "What is (3 + 7) * (10 - 4)? Use the tools step by step.",
        "Now divide that result by 5 using the divide tool.",
        "What was the result before dividing? Respond with just the number.",
    ];

    let mut history: Vec<Message> = Vec::new();

    for (turn_idx, question) in questions.iter().enumerate() {
        let turn = turn_idx + 1;
        eprintln!("━━━ Turn {turn}: {question}");
        eprintln!("    (history: {} messages)", history.len());

        let input = LoopInput::start(question.to_string()).history(history.clone());
        let stream = agent.chat(input).await?;
        let mut stream = std::pin::pin!(stream);

        while let Some(event) = stream.next().await {
            match event {
                AgentEvent::TextDelta(text) => {
                    print!("{text}");
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                }
                AgentEvent::ToolResult { name, result, .. } => {
                    eprintln!("  ✓ {name} → {result}");
                }
                AgentEvent::Usage {
                    prompt_tokens,
                    completion_tokens,
                } => {
                    eprintln!("  [tokens ↑{prompt_tokens} ↓{completion_tokens}]");
                }
                AgentEvent::NewMessages(msgs) => {
                    eprintln!("  [+{} messages persisted]", msgs.len());
                    history.extend(msgs);
                }
                AgentEvent::Done => {
                    println!();
                }
                AgentEvent::Error(e) => {
                    eprintln!("  ✗ {e}");
                }
                _ => {}
            }
        }

        eprintln!("    total history: {} messages", history.len());
        eprintln!();
    }

    eprintln!("═══ All {n} turns complete, {m} messages in history ═══",
        n = questions.len(),
        m = history.len(),
    );

    Ok(())
}
