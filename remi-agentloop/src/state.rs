use async_stream::stream;
use futures::{Stream, StreamExt};

use crate::error::AgentError;
use crate::model::ChatModel;
use crate::tool::registry::ToolRegistry;
use crate::tool::{ToolOutput, ToolResult};
use crate::tracing::DynTracer;
use crate::types::{
    AgentEvent, ChatRequest, ChatResponseChunk, InterruptInfo, Message,
    ParsedToolCall, RunId, ThreadId, ToolCallMessage, ToolCallResult,
    FunctionCall,
};

/// Internal: accumulates streaming tool call fragments
struct ToolCallAccumulator {
    #[allow(dead_code)]
    index: usize,
    id: String,
    name: String,
    arguments: String,
}

// ── AgentLoop ─────────────────────────────────────────────────────────────────

pub struct AgentLoop<'a, M: ChatModel> {
    model: &'a M,
    tools: &'a ToolRegistry,
    tracer: Option<&'a dyn DynTracer>,
    thread_id: ThreadId,
    run_id: RunId,
    metadata: Option<serde_json::Value>,
    messages: Vec<Message>,
    max_turns: usize,
    model_name: String,
}

impl<'a, M: ChatModel> AgentLoop<'a, M> {
    pub fn new(
        model: &'a M,
        tools: &'a ToolRegistry,
        messages: Vec<Message>,
        max_turns: usize,
        model_name: impl Into<String>,
    ) -> Self {
        Self {
            model,
            tools,
            tracer: None,
            thread_id: ThreadId::new(),
            run_id: RunId::new(),
            metadata: None,
            messages,
            max_turns,
            model_name: model_name.into(),
        }
    }

    pub fn with_tracer(mut self, tracer: &'a dyn DynTracer) -> Self {
        self.tracer = Some(tracer);
        self
    }

    pub fn with_thread(mut self, thread_id: ThreadId) -> Self {
        self.thread_id = thread_id;
        self
    }

    pub fn with_run_id(mut self, run_id: RunId) -> Self {
        self.run_id = run_id;
        self
    }

    pub fn with_metadata(mut self, meta: serde_json::Value) -> Self {
        self.metadata = Some(meta);
        self
    }

    /// Primary entry point — returns a Stream of AgentEvents starting with RunStart
    pub fn into_stream(self) -> impl Stream<Item = AgentEvent> + 'a {
        let run_id = self.run_id.clone();
        let thread_id = self.thread_id.clone();
        let metadata = self.metadata.clone();
        let max_turns = self.max_turns;
        let model_name = self.model_name.clone();
        let model = self.model;
        let tools = self.tools;
        let tracer = self.tracer;
        let mut messages = self.messages;

        stream! {
            // First event: RunStart
            yield AgentEvent::RunStart {
                thread_id: thread_id.clone(),
                run_id: run_id.clone(),
                metadata: metadata.clone(),
            };

            // Optional: notify tracer
            if let Some(t) = tracer {
                t.on_turn_start(&crate::tracing::TurnStartTrace {
                    run_id: run_id.clone(),
                    turn: 0,
                    timestamp: chrono::Utc::now(),
                }).await;
            }

            for turn in 0..max_turns {
                // ── CallingModel ──────────────────────────────────────────
                let tool_defs = if tools.is_empty() { None } else { Some(tools.definitions()) };

                let request = ChatRequest {
                    model: model_name.clone(),
                    messages: messages.clone(),
                    tools: tool_defs,
                    temperature: None,
                    max_tokens: None,
                    stream: true,
                    metadata: metadata.clone(),
                };

                if let Some(t) = tracer {
                    t.on_model_start(&crate::tracing::ModelStartTrace {
                        run_id: run_id.clone(),
                        turn,
                        model: model_name.clone(),
                        messages: messages.clone(),
                        tools: tools.definitions().iter().map(|d| d.function.name.clone()).collect(),
                        timestamp: chrono::Utc::now(),
                    }).await;
                }

                let chat_stream = match model.chat(request).await {
                    Ok(s) => s,
                    Err(e) => {
                        yield AgentEvent::Error(e);
                        return;
                    }
                };
                let mut chat_stream = std::pin::pin!(chat_stream);

                // ── Streaming ─────────────────────────────────────────────
                let mut tool_accumulator: Vec<ToolCallAccumulator> = Vec::new();
                // index → position in tool_accumulator
                let mut index_map: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
                let mut text_parts: Vec<String> = Vec::new();
                let mut prompt_tokens = 0u32;
                let mut completion_tokens = 0u32;

                while let Some(chunk) = chat_stream.next().await {
                    match chunk {
                        ChatResponseChunk::Delta { content, .. } => {
                            text_parts.push(content.clone());
                            yield AgentEvent::TextDelta(content);
                        }
                        ChatResponseChunk::ToolCallStart { index, id, name } => {
                            yield AgentEvent::ToolCallStart { id: id.clone(), name: name.clone() };
                            let pos = tool_accumulator.len();
                            tool_accumulator.push(ToolCallAccumulator { index, id, name, arguments: String::new() });
                            index_map.insert(index, pos);
                        }
                        ChatResponseChunk::ToolCallDelta { index, arguments_delta } => {
                            if let Some(&pos) = index_map.get(&index) {
                                let tc = &mut tool_accumulator[pos];
                                yield AgentEvent::ToolCallArgumentsDelta {
                                    id: tc.id.clone(),
                                    delta: arguments_delta.clone(),
                                };
                                tc.arguments.push_str(&arguments_delta);
                            }
                        }
                        ChatResponseChunk::Usage { prompt_tokens: p, completion_tokens: c, .. } => {
                            prompt_tokens = p;
                            completion_tokens = c;
                            yield AgentEvent::Usage { prompt_tokens: p, completion_tokens: c };
                        }
                        ChatResponseChunk::Done => break,
                    }
                }

                if let Some(t) = tracer {
                    t.on_model_end(&crate::tracing::ModelEndTrace {
                        run_id: run_id.clone(),
                        turn,
                        response_text: if text_parts.is_empty() { None } else { Some(text_parts.join("")) },
                        tool_calls: vec![],
                        prompt_tokens,
                        completion_tokens,
                        duration: std::time::Duration::ZERO,
                        timestamp: chrono::Utc::now(),
                    }).await;
                }

                // No tool calls → done
                if tool_accumulator.is_empty() {
                    // Append assistant message
                    let text = text_parts.join("");
                    messages.push(Message::assistant(&text));
                    yield AgentEvent::Done;
                    return;
                }

                // Build assistant message with tool_calls
                let tool_call_messages: Vec<ToolCallMessage> = tool_accumulator.iter().map(|tc| {
                    ToolCallMessage {
                        id: tc.id.clone(),
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: tc.name.clone(),
                            arguments: tc.arguments.clone(),
                        },
                    }
                }).collect();
                let text = text_parts.join("");
                messages.push(Message::assistant_with_tool_calls(text, tool_call_messages));

                // Parse tool calls
                let parsed: Vec<ParsedToolCall> = tool_accumulator.into_iter()
                    .map(|tc| ParsedToolCall {
                        id: tc.id,
                        name: tc.name,
                        arguments: serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null),
                    })
                    .collect();

                // ── ExecutingTools ────────────────────────────────────────
                let tool_results = tools.execute_parallel(&parsed).await;

                let mut completed_results: Vec<ToolCallResult> = Vec::new();
                let mut pending_interrupts: Vec<InterruptInfo> = Vec::new();

                for (tool_call_id, tool_result) in tool_results {
                    let tc = parsed.iter().find(|p| p.id == tool_call_id).unwrap();

                    if let Some(t) = tracer {
                        t.on_tool_start(&crate::tracing::ToolStartTrace {
                            run_id: run_id.clone(),
                            turn,
                            tool_call_id: tool_call_id.clone(),
                            tool_name: tc.name.clone(),
                            arguments: tc.arguments.clone(),
                            timestamp: chrono::Utc::now(),
                        }).await;
                    }

                    match tool_result {
                        Err(e) => {
                            let msg = e.to_string();
                            yield AgentEvent::Error(e);
                            completed_results.push(ToolCallResult {
                                id: tool_call_id,
                                name: tc.name.clone(),
                                result: format!("error: {}", msg),
                            });
                        }
                        Ok(ToolResult::Interrupt(req)) => {
                            if let Some(t) = tracer {
                                t.on_tool_end(&crate::tracing::ToolEndTrace {
                                    run_id: run_id.clone(),
                                    turn,
                                    tool_call_id: tool_call_id.clone(),
                                    tool_name: tc.name.clone(),
                                    result: None,
                                    interrupted: true,
                                    duration: std::time::Duration::ZERO,
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
                        Ok(ToolResult::Output(mut tool_stream)) => {
                            let mut last_result = None;
                            while let Some(output) = tool_stream.next().await {
                                match output {
                                    ToolOutput::Delta(delta) => {
                                        yield AgentEvent::ToolDelta {
                                            id: tool_call_id.clone(),
                                            name: tc.name.clone(),
                                            delta,
                                        };
                                    }
                                    ToolOutput::Result(result) => {
                                        yield AgentEvent::ToolResult {
                                            id: tool_call_id.clone(),
                                            name: tc.name.clone(),
                                            result: result.clone(),
                                        };
                                        last_result = Some(result);
                                    }
                                }
                            }
                            if let Some(t) = tracer {
                                t.on_tool_end(&crate::tracing::ToolEndTrace {
                                    run_id: run_id.clone(),
                                    turn,
                                    tool_call_id: tool_call_id.clone(),
                                    tool_name: tc.name.clone(),
                                    result: last_result.clone(),
                                    interrupted: false,
                                    duration: std::time::Duration::ZERO,
                                    timestamp: chrono::Utc::now(),
                                }).await;
                            }
                            if let Some(result) = last_result {
                                completed_results.push(ToolCallResult {
                                    id: tool_call_id,
                                    name: tc.name.clone(),
                                    result,
                                });
                            }
                        }
                    }
                }

                // ── Check for interrupts ──────────────────────────────────
                if !pending_interrupts.is_empty() {
                    if let Some(t) = tracer {
                        t.on_interrupt(&crate::tracing::InterruptTrace {
                            run_id: run_id.clone(),
                            interrupts: pending_interrupts.clone(),
                            timestamp: chrono::Utc::now(),
                        }).await;
                    }
                    yield AgentEvent::Interrupt { interrupts: pending_interrupts };
                    // Stream ends here — caller must resume()
                    return;
                }

                // ── No interrupts: append tool results to messages ────────
                for tr in &completed_results {
                    messages.push(Message::tool_result(&tr.id, &tr.result));
                }

                yield AgentEvent::TurnStart { turn: turn + 1 };

                if let Some(t) = tracer {
                    t.on_turn_start(&crate::tracing::TurnStartTrace {
                        run_id: run_id.clone(),
                        turn: turn + 1,
                        timestamp: chrono::Utc::now(),
                    }).await;
                }
            }

            // Max turns exceeded
            yield AgentEvent::Error(AgentError::MaxTurnsExceeded { max: max_turns });
        }
    }
}
