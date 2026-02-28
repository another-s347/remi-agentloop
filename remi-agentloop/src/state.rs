//! Pure, stateless step function and associated types.
//!
//! The agent loop is decomposed into a single primitive:
//!
//! ```ignore
//! step(state, action, &model) -> Stream<StepEvent>
//! ```
//!
//! Each call to [`step`] makes exactly **one** model request. The stream
//! yields real-time deltas and terminates with a *terminal event* that
//! carries the updated [`AgentState`]:
//!
//! - [`StepEvent::Done`] — model responded with text only.
//! - [`StepEvent::NeedToolExecution`] — model requested tool calls; caller
//!   executes tools externally and feeds results back via [`Action::ToolResults`].
//! - [`StepEvent::Error`] — an error occurred.
//!
//! Tools are **never** called inside `step()`. The caller (e.g. [`BuiltAgent`](crate::builder::BuiltAgent))
//! is responsible for tool execution, interrupt handling, and turn counting.

use async_stream::stream;
use futures::{Stream, StreamExt};

use crate::error::AgentError;
use crate::model::ChatModel;
use crate::tool::ToolDefinition;
use crate::types::{
    ChatRequest, ChatResponseChunk, Content, Message, ParsedToolCall,
    StreamOptions, ToolCallMessage, ToolCallOutcome, FunctionCall,
    ThreadId, RunId,
};

// ── AgentState ────────────────────────────────────────────────────────────────

/// Fully serialisable snapshot of the entire agent state.
///
/// Can be persisted, transferred across processes, or inspected by the caller
/// between steps. The `step()` function consumes an `AgentState` and yields
/// a new one in the terminal [`StepEvent`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentState {
    /// Conversation messages (system, user, assistant, tool results).
    pub messages: Vec<Message>,

    /// System prompt prepended when building the chat request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    /// Tool definitions advertised to the model.
    #[serde(default)]
    pub tool_definitions: Vec<ToolDefinition>,

    /// Model & request configuration.
    pub config: StepConfig,

    /// Thread identifier (caller-assigned).
    pub thread_id: ThreadId,

    /// Run identifier (caller-assigned).
    pub run_id: RunId,

    /// Current turn counter (incremented by the caller / outer loop).
    pub turn: usize,

    /// Current phase — indicates what action the caller should take next.
    pub phase: AgentPhase,

    /// Opaque user-defined state carried alongside the conversation.
    #[serde(default)]
    pub user_state: serde_json::Value,
}

impl AgentState {
    /// Create a minimal ready state with required config.
    pub fn new(config: StepConfig) -> Self {
        Self {
            messages: Vec::new(),
            system_prompt: None,
            tool_definitions: Vec::new(),
            config,
            thread_id: ThreadId::new(),
            run_id: RunId::new(),
            turn: 0,
            phase: AgentPhase::Ready,
            user_state: serde_json::Value::Null,
        }
    }

    /// Builder helper: set system prompt.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Builder helper: set tool definitions.
    pub fn with_tool_definitions(mut self, defs: Vec<ToolDefinition>) -> Self {
        self.tool_definitions = defs;
        self
    }

    /// Builder helper: set thread id.
    pub fn with_thread_id(mut self, id: ThreadId) -> Self {
        self.thread_id = id;
        self
    }

    /// Builder helper: set run id.
    pub fn with_run_id(mut self, id: RunId) -> Self {
        self.run_id = id;
        self
    }

    /// Builder helper: set initial messages.
    pub fn with_messages(mut self, msgs: Vec<Message>) -> Self {
        self.messages = msgs;
        self
    }

    /// Builder helper: set user state.
    pub fn with_user_state(mut self, state: serde_json::Value) -> Self {
        self.user_state = state;
        self
    }
}

// ── StepConfig ────────────────────────────────────────────────────────────────

/// Configuration for a single step (model call).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StepConfig {
    /// Model name (e.g. `"gpt-4o"`, `"kimi-k2.5"`).
    pub model: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl StepConfig {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            temperature: None,
            max_tokens: None,
            metadata: None,
        }
    }
}

// ── AgentPhase ────────────────────────────────────────────────────────────────

/// Indicates what the caller should do next.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum AgentPhase {
    /// Ready for a new step (user message or tool results).
    Ready,
    /// Model requested tool calls — caller should execute and respond.
    AwaitingToolExecution {
        tool_calls: Vec<ParsedToolCall>,
    },
    /// Conversation complete.
    Done,
    /// An error occurred.
    Error,
}

// ── Action ────────────────────────────────────────────────────────────────────

/// Caller-supplied input to [`step()`].
#[derive(Debug, Clone)]
pub enum Action {
    /// Start with a plain-text user message.
    UserMessage(String),
    /// Start with rich (multimodal) content.
    UserContent(Content),
    /// Feed back tool execution results (response to `NeedToolExecution`).
    ToolResults(Vec<ToolCallOutcome>),
}

// ── StepEvent ─────────────────────────────────────────────────────────────────

/// Events streamed from a single [`step()`] call.
///
/// The stream always ends with exactly one *terminal* event (`Done`,
/// `NeedToolExecution`, or `Error`) which carries the updated [`AgentState`].
#[derive(Debug)]
pub enum StepEvent {
    // ── streaming deltas ──
    TextDelta(String),
    ToolCallStart { id: String, name: String },
    ToolCallArgumentsDelta { id: String, delta: String },
    Usage { prompt_tokens: u32, completion_tokens: u32 },

    // ── terminal (exactly one, always last) ──
    /// Model responded with text; no tool calls.
    Done { state: AgentState },
    /// Model requested tool calls. Execute externally, then call
    /// `step(state, Action::ToolResults(..), model)`.
    NeedToolExecution {
        state: AgentState,
        tool_calls: Vec<ParsedToolCall>,
    },
    /// An error occurred.
    Error { state: AgentState, error: AgentError },
}

// ── Internal accumulator ──────────────────────────────────────────────────────

struct ToolCallAccumulator {
    #[allow(dead_code)]
    index: usize,
    id: String,
    name: String,
    arguments: String,
}

// ── step() ────────────────────────────────────────────────────────────────────

/// Pure, stateless step: one model call, streaming deltas, terminal event
/// with the new [`AgentState`].
///
/// # Panics
/// Does **not** panic. Model or transport errors are reported via
/// [`StepEvent::Error`].
pub fn step<M: ChatModel>(
    state: AgentState,
    action: Action,
    model: &M,
) -> impl Stream<Item = StepEvent> + '_ {
    let mut state = state;

    stream! {
        // ── 1. Apply the action to mutate state ──────────────────────
        match action {
            Action::UserMessage(text) => {
                // Ensure system prompt is present as the first message
                if let Some(ref sys) = state.system_prompt {
                    if !state.messages.first().is_some_and(|m| matches!(m.role, crate::types::Role::System)) {
                        state.messages.insert(0, Message::system(sys));
                    }
                }
                state.messages.push(Message::user(&text));
            }
            Action::UserContent(content) => {
                if let Some(ref sys) = state.system_prompt {
                    if !state.messages.first().is_some_and(|m| matches!(m.role, crate::types::Role::System)) {
                        state.messages.insert(0, Message::system(sys));
                    }
                }
                state.messages.push(Message {
                    id: crate::types::MessageId::new(),
                    role: crate::types::Role::User,
                    content,
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
            Action::ToolResults(outcomes) => {
                for outcome in outcomes {
                    match outcome {
                        ToolCallOutcome::Result { tool_call_id, result, .. } => {
                            state.messages.push(Message::tool_result(&tool_call_id, &result));
                        }
                        ToolCallOutcome::Error { tool_call_id, error, .. } => {
                            state.messages.push(Message::tool_result(&tool_call_id, &format!("error: {error}")));
                        }
                    }
                }
            }
        }

        // ── 2. Build ChatRequest ─────────────────────────────────────
        let tool_defs = if state.tool_definitions.is_empty() {
            None
        } else {
            Some(state.tool_definitions.clone())
        };

        let request = ChatRequest {
            model: state.config.model.clone(),
            messages: state.messages.clone(),
            tools: tool_defs,
            temperature: state.config.temperature,
            max_tokens: state.config.max_tokens,
            stream: true,
            stream_options: Some(StreamOptions { include_usage: true }),
            metadata: state.config.metadata.clone(),
        };

        // ── 3. Call model ────────────────────────────────────────────
        let chat_stream = match model.chat(request).await {
            Ok(s) => s,
            Err(e) => {
                state.phase = AgentPhase::Error;
                yield StepEvent::Error { state, error: e };
                return;
            }
        };
        let mut chat_stream = std::pin::pin!(chat_stream);

        // ── 4. Stream model chunks ───────────────────────────────────
        let mut tool_accumulators: Vec<ToolCallAccumulator> = Vec::new();
        let mut index_map: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        let mut text_parts: Vec<String> = Vec::new();

        while let Some(chunk) = chat_stream.next().await {
            match chunk {
                ChatResponseChunk::Delta { content, .. } => {
                    text_parts.push(content.clone());
                    yield StepEvent::TextDelta(content);
                }
                ChatResponseChunk::ToolCallStart { index, id, name } => {
                    yield StepEvent::ToolCallStart { id: id.clone(), name: name.clone() };
                    let pos = tool_accumulators.len();
                    tool_accumulators.push(ToolCallAccumulator { index, id, name, arguments: String::new() });
                    index_map.insert(index, pos);
                }
                ChatResponseChunk::ToolCallDelta { index, arguments_delta } => {
                    if let Some(&pos) = index_map.get(&index) {
                        let tc = &mut tool_accumulators[pos];
                        yield StepEvent::ToolCallArgumentsDelta {
                            id: tc.id.clone(),
                            delta: arguments_delta.clone(),
                        };
                        tc.arguments.push_str(&arguments_delta);
                    }
                }
                ChatResponseChunk::Usage { prompt_tokens, completion_tokens, .. } => {
                    yield StepEvent::Usage { prompt_tokens, completion_tokens };
                }
                ChatResponseChunk::Done => break,
            }
        }

        // ── 5. Terminal event ────────────────────────────────────────

        if tool_accumulators.is_empty() {
            // No tool calls — assistant text response
            let text = text_parts.join("");
            state.messages.push(Message::assistant(&text));
            state.phase = AgentPhase::Done;
            yield StepEvent::Done { state };
        } else {
            // Tool calls — build assistant message with tool_calls
            let tool_call_messages: Vec<ToolCallMessage> = tool_accumulators.iter().map(|tc| {
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
            state.messages.push(Message::assistant_with_tool_calls(text, tool_call_messages));

            let parsed: Vec<ParsedToolCall> = tool_accumulators.into_iter()
                .map(|tc| ParsedToolCall {
                    id: tc.id,
                    name: tc.name,
                    arguments: serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null),
                })
                .collect();

            state.phase = AgentPhase::AwaitingToolExecution { tool_calls: parsed.clone() };
            yield StepEvent::NeedToolExecution { state, tool_calls: parsed };
        }
    }
}
