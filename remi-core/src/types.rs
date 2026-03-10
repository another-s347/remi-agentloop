use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

fn uuid_v4() -> String {
    Uuid::new_v4().to_string()
}

// ── Identifiers ──────────────────────────────────────────────────────────────

/// 会话线程 ID——一个 Thread 包含多轮 Run，多条 Message
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThreadId(pub String);

/// 单次运行 ID——一次 agent.chat() 调用，interrupt/resume 保持不变
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

/// 消息 ID——标识 Thread 中的每条消息
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub String);

/// 中断标识符——一个 tool 的一次 interrupt
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InterruptId(pub String);

impl ThreadId {
    pub fn new() -> Self {
        Self(uuid_v4())
    }
}
impl RunId {
    pub fn new() -> Self {
        Self(uuid_v4())
    }
}
impl MessageId {
    pub fn new() -> Self {
        Self(uuid_v4())
    }
}
impl InterruptId {
    pub fn new() -> Self {
        Self(uuid_v4())
    }
}

impl Default for ThreadId {
    fn default() -> Self {
        Self::new()
    }
}
impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}
impl Default for MessageId {
    fn default() -> Self {
        Self::new()
    }
}
impl Default for InterruptId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl fmt::Display for InterruptId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ── Multimodal Content ────────────────────────────────────────────────────────

/// 消息内容——兼容 OpenAI content 字段
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl Content {
    pub fn text(s: impl Into<String>) -> Self {
        Content::Text(s.into())
    }
    pub fn parts(parts: Vec<ContentPart>) -> Self {
        Content::Parts(parts)
    }

    pub fn text_content(&self) -> String {
        match self {
            Content::Text(s) => s.clone(),
            Content::Parts(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    pub fn is_multimodal(&self) -> bool {
        matches!(self, Content::Parts(parts) if parts.iter().any(|p| !matches!(p, ContentPart::Text { .. })))
    }
}

/// 单个内容部分——对应 OpenAI 多模态 content part
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlDetail },

    #[serde(rename = "image_base64")]
    ImageBase64 { media_type: String, data: String },

    #[serde(rename = "input_audio")]
    Audio { input_audio: AudioDetail },

    #[serde(rename = "file")]
    File {
        file_id: Option<String>,
        filename: Option<String>,
        media_type: Option<String>,
        data: Option<String>,
    },
}

impl ContentPart {
    pub fn text(s: impl Into<String>) -> Self {
        ContentPart::Text { text: s.into() }
    }
    pub fn image_url(url: impl Into<String>) -> Self {
        ContentPart::ImageUrl {
            image_url: ImageUrlDetail {
                url: url.into(),
                detail: None,
            },
        }
    }
    pub fn image_base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        ContentPart::ImageBase64 {
            media_type: media_type.into(),
            data: data.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrlDetail {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDetail {
    pub data: String,
    pub format: String,
}

// ── Role & Message ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub id: MessageId,
    pub role: Role,
    pub content: Content,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Chain-of-thought / reasoning text returned by thinking models (e.g. Kimi K2.5).
    /// Must be echoed back verbatim when replaying the conversation history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::User,
            content: Content::text(text),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::System,
            content: Content::text(text),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::Assistant,
            content: Content::text(text),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn assistant_with_tool_calls(
        text: impl Into<String>,
        tool_calls: Vec<ToolCallMessage>,
        reasoning_content: Option<String>,
    ) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::Assistant,
            content: Content::text(text),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            reasoning_content,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, result: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::Tool,
            content: Content::text(result),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            reasoning_content: None,
        }
    }

    /// Tool result with rich content (text and/or images).
    pub fn tool_result_content(tool_call_id: impl Into<String>, content: Content) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::Tool,
            content,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            reasoning_content: None,
        }
    }

    pub fn user_multimodal(parts: Vec<ContentPart>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::User,
            content: Content::parts(parts),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallMessage {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

// ── ChatRequest / ChatResponseChunk ──────────────────────────────────────────

use crate::tool::ToolDefinition;

#[derive(Debug, Clone, Serialize)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub enum ChatResponseChunk {
    Delta {
        content: String,
        role: Option<Role>,
    },
    /// Chain-of-thought / thinking content from reasoning models (e.g. Kimi K2.5, DeepSeek-R1).
    ReasoningDelta {
        content: String,
    },
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
    },
    ToolCallDelta {
        index: usize,
        arguments_delta: String,
    },
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
    },
    Done,
}

// ── AgentEvent ────────────────────────────────────────────────────────────────

use crate::error::AgentError;

/// Agent loop 对外 yield 的事件
#[derive(Debug, Clone)]
pub enum AgentEvent {
    RunStart {
        thread_id: ThreadId,
        run_id: RunId,
        metadata: Option<serde_json::Value>,
    },
    TextDelta(String),
    /// Emitted once when a thinking model begins its chain-of-thought.
    /// All events until `ThinkingEnd` occur conceptually inside the thinking phase.
    ThinkingStart,
    /// Emitted when the thinking phase ends. Carries the full accumulated reasoning text.
    ThinkingEnd {
        content: String,
    },
    ToolCallStart {
        id: String,
        name: String,
    },
    ToolCallArgumentsDelta {
        id: String,
        delta: String,
    },
    ToolDelta {
        id: String,
        name: String,
        delta: String,
    },
    ToolResult {
        id: String,
        name: String,
        result: String,
    },
    Interrupt {
        interrupts: Vec<InterruptInfo>,
    },
    TurnStart {
        turn: usize,
    },
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
    },
    Done,
    /// The run was cancelled by the user.  A `Cancelled` checkpoint has been
    /// saved; the conversation can be resumed from where it was interrupted.
    Cancelled,
    Error(AgentError),
    /// Full state checkpoint emitted at key lifecycle boundaries.
    /// Outer layers (e.g. `BuiltAgent`) intercept this for durable persistence
    /// and filter it out before reaching the consumer.
    ///
    /// Contains everything needed to resume execution after a crash or restart.
    Checkpoint(crate::checkpoint::Checkpoint),
    /// Tool calls that the inner agent loop cannot execute (not in its registry).
    /// The outer layer should execute these externally, then resume via
    /// `AgentLoop::run(state, Action::ToolResults(all_outcomes), false)`.
    ///
    /// `completed_results` contains outcomes of tools that **were** executed
    /// internally by this loop. The outer layer must merge its own results
    /// with these before resuming.
    NeedToolExecution {
        state: crate::state::AgentState,
        tool_calls: Vec<ParsedToolCall>,
        completed_results: Vec<ToolCallOutcome>,
    },
}

/// 单个中断的详情
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptInfo {
    pub interrupt_id: InterruptId,
    pub tool_call_id: String,
    pub tool_name: String,
    pub kind: String,
    pub data: serde_json::Value,
}

/// 恢复中断时传入的数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumePayload {
    pub interrupt_id: InterruptId,
    pub result: serde_json::Value,
}

// ── Internal loop types (pub(crate)) ─────────────────────────────────────────

/// Parsed and fully accumulated tool call ready for execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Single tool call execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub id: String,
    pub name: String,
    pub result: String,
}

/// Outcome of executing a tool externally — fed back into [`step()`](crate::state::step)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolCallOutcome {
    /// Tool executed successfully (content may include text and/or images)
    Result {
        tool_call_id: String,
        tool_name: String,
        content: Content,
    },
    /// Tool execution failed
    Error {
        tool_call_id: String,
        tool_name: String,
        error: String,
    },
}

// ── LoopInput ─────────────────────────────────────────────────────────────────

/// Unified input for `Agent::chat()` — used by `AgentLoop`, composable layers,
/// and the protocol/transport layer.
///
/// Merges the previous `LoopInput` and `ProtocolRequest` into a single
/// serialisable type that supports:
/// - Starting a new turn with text or multimodal content
/// - Resuming after `NeedToolExecution`
/// - Protocol-level overrides (model, temperature, max_tokens, metadata)
///
/// ```ignore
/// // Start a new conversation (String converts automatically):
/// agent.chat("hello".into()).await?;
///
/// // Start with multimodal content:
/// agent.chat(Content::parts(vec![
///     ContentPart::text("describe this image"),
///     ContentPart::image_url("https://example.com/img.png"),
/// ]).into()).await?;
///
/// // Start with history + extra tool definitions + overrides:
/// agent.chat(
///     LoopInput::start("hello")
///         .history(msgs)
///         .extra_tools(defs)
///         .model("gpt-4o")
///         .temperature(0.5)
/// ).await?;
///
/// // Resume after NeedToolExecution:
/// agent.chat(LoopInput::resume(state, outcomes)).await?;
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LoopInput {
    /// Start a new conversation turn
    #[serde(rename = "start")]
    Start {
        /// User message content — text or multimodal
        content: Content,
        /// Conversation history from prior turns
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        history: Vec<Message>,
        /// Additional tool definitions injected by outer layers
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extra_tools: Vec<crate::tool::ToolDefinition>,
        /// Override model name for this request
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Override temperature for this request
        #[serde(skip_serializing_if = "Option::is_none")]
        temperature: Option<f64>,
        /// Override max tokens for this request
        #[serde(skip_serializing_if = "Option::is_none")]
        max_tokens: Option<u32>,
        /// Request metadata
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },
    /// Resume from a `NeedToolExecution` with completed tool results
    #[serde(rename = "resume")]
    Resume {
        state: crate::state::AgentState,
        results: Vec<ToolCallOutcome>,
    },
    /// Cancel an in-progress run.  Produces a `Cancelled` checkpoint so the
    /// conversation can be resumed later.
    #[serde(rename = "cancel")]
    Cancel { state: crate::state::AgentState },
}

impl LoopInput {
    /// Create a `Start` input with a text message.
    pub fn start(msg: impl Into<String>) -> Self {
        Self::Start {
            content: Content::text(msg),
            history: vec![],
            extra_tools: vec![],
            model: None,
            temperature: None,
            max_tokens: None,
            metadata: None,
        }
    }

    /// Create a `Start` input with multimodal content.
    pub fn start_content(content: Content) -> Self {
        Self::Start {
            content,
            history: vec![],
            extra_tools: vec![],
            model: None,
            temperature: None,
            max_tokens: None,
            metadata: None,
        }
    }

    /// Create a `Resume` input from state + tool results.
    pub fn resume(state: crate::state::AgentState, results: Vec<ToolCallOutcome>) -> Self {
        Self::Resume { state, results }
    }

    /// Create a `Cancel` input to abort a running conversation.
    pub fn cancel(state: crate::state::AgentState) -> Self {
        Self::Cancel { state }
    }

    /// Builder: attach conversation history (only applies to `Start`).
    pub fn history(mut self, msgs: Vec<Message>) -> Self {
        if let Self::Start { history, .. } = &mut self {
            *history = msgs;
        }
        self
    }

    /// Builder: attach extra tool definitions (only applies to `Start`).
    pub fn extra_tools(mut self, defs: Vec<crate::tool::ToolDefinition>) -> Self {
        if let Self::Start { extra_tools, .. } = &mut self {
            *extra_tools = defs;
        }
        self
    }

    /// Builder: override model name (only applies to `Start`).
    pub fn model(mut self, m: impl Into<String>) -> Self {
        if let Self::Start { model, .. } = &mut self {
            *model = Some(m.into());
        }
        self
    }

    /// Builder: override temperature (only applies to `Start`).
    pub fn temperature(mut self, t: f64) -> Self {
        if let Self::Start { temperature, .. } = &mut self {
            *temperature = Some(t);
        }
        self
    }

    /// Builder: override max tokens (only applies to `Start`).
    pub fn max_tokens(mut self, n: u32) -> Self {
        if let Self::Start { max_tokens, .. } = &mut self {
            *max_tokens = Some(n);
        }
        self
    }

    /// Builder: set metadata (only applies to `Start`).
    pub fn metadata(mut self, v: serde_json::Value) -> Self {
        if let Self::Start { metadata, .. } = &mut self {
            *metadata = Some(v);
        }
        self
    }
}

impl From<String> for LoopInput {
    fn from(s: String) -> Self {
        Self::start(s)
    }
}

impl From<&str> for LoopInput {
    fn from(s: &str) -> Self {
        Self::start(s)
    }
}

impl From<Content> for LoopInput {
    fn from(c: Content) -> Self {
        Self::start_content(c)
    }
}

// ── ChatInput ─────────────────────────────────────────────────────────────────

/// Unified input for `chat_in_thread` — covers both new messages and resume from interrupt.
///
/// ```ignore
/// // New user message (String converts automatically):
/// agent.chat_in_thread(&tid, "hello").await?;
///
/// // Resume from interrupt:
/// agent.chat_in_thread(&tid, ChatInput::Resume {
///     run_id,
///     completed_results: vec![],
///     pending_interrupts: interrupts,
///     payloads: vec![payload],
/// }).await?;
/// ```
#[derive(Debug, Clone)]
pub enum ChatInput {
    /// A new user message
    Message(String),
    /// Resume a previously interrupted run
    Resume {
        run_id: RunId,
        /// Tool calls that completed normally (before the interrupt)
        completed_results: Vec<ToolCallResult>,
        /// The interrupt(s) that were returned by the agent
        pending_interrupts: Vec<InterruptInfo>,
        /// User-provided payloads resolving each interrupt
        payloads: Vec<ResumePayload>,
    },
    /// Cancel an in-progress run.  Saves a `Cancelled` checkpoint and
    /// yields `AgentEvent::Cancelled` so the conversation can be resumed later.
    Cancel { run_id: RunId },
}

impl From<String> for ChatInput {
    fn from(s: String) -> Self {
        ChatInput::Message(s)
    }
}

impl From<&str> for ChatInput {
    fn from(s: &str) -> Self {
        ChatInput::Message(s.to_string())
    }
}
