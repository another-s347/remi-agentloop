//! Serde-compatible type definitions for the remi-agentloop WASM guest interface.
//!
//! These types mirror the host-side types in `remi-agentloop` and are
//! serialization-compatible via JSON. They are intentionally kept free of
//! async / platform-specific dependencies so they compile to `wasm32`.

use serde::{Deserialize, Serialize};

// ── Identifiers ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThreadId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InterruptId(pub String);

impl std::fmt::Display for ThreadId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::fmt::Display for MessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::fmt::Display for InterruptId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ── Multimodal Content ───────────────────────────────────────────────────────

/// Message content — compatible with OpenAI `content` field.
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

    /// Extract all text content, ignoring non-text parts.
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

/// Individual content part — corresponds to OpenAI multimodal content part.
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

// ── Role & Message ───────────────────────────────────────────────────────────

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
    pub id: MessageId,
    pub role: Role,
    pub content: Content,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
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

// ── Tool Definition ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

// ── Agent State ──────────────────────────────────────────────────────────────

/// Fully serialisable snapshot of agent state (mirrors host-side `AgentState`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub tool_definitions: Vec<ToolDefinition>,
    pub config: StepConfig,
    pub thread_id: ThreadId,
    pub run_id: RunId,
    pub turn: usize,
    pub phase: AgentPhase,
    #[serde(default)]
    pub user_state: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepConfig {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentPhase {
    Ready,
    AwaitingToolExecution { tool_calls: Vec<ParsedToolCall> },
    Done,
    Error,
}

// ── Tool Calls & Outcomes ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Single tool execution outcome — fed back via `LoopInput::Resume`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolCallOutcome {
    Result {
        tool_call_id: String,
        tool_name: String,
        result: String,
    },
    Error {
        tool_call_id: String,
        tool_name: String,
        error: String,
    },
}

// ── Interrupt ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptInfo {
    pub interrupt_id: InterruptId,
    pub tool_call_id: String,
    pub tool_name: String,
    pub kind: String,
    pub data: serde_json::Value,
}

// ── LoopInput ────────────────────────────────────────────────────────────────

/// Unified input for agent chat — deserialized from JSON in the WIT `chat` function.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LoopInput {
    /// Start a new conversation turn.
    #[serde(rename = "start")]
    Start {
        content: Content,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        history: Vec<Message>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extra_tools: Vec<ToolDefinition>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        temperature: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_tokens: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },
    /// Resume from a `NeedToolExecution` with completed tool results.
    #[serde(rename = "resume")]
    Resume {
        state: AgentState,
        results: Vec<ToolCallOutcome>,
    },
}

impl LoopInput {
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

// ── ProtocolEvent ────────────────────────────────────────────────────────────

/// Protocol events emitted by the guest via the WIT `emit` import.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProtocolEvent {
    #[serde(rename = "run_start")]
    RunStart {
        thread_id: String,
        run_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },

    #[serde(rename = "delta")]
    Delta {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },

    #[serde(rename = "tool_call_start")]
    ToolCallStart { id: String, name: String },

    #[serde(rename = "tool_call_delta")]
    ToolCallDelta { id: String, arguments_delta: String },

    #[serde(rename = "tool_delta")]
    ToolDelta {
        id: String,
        name: String,
        delta: String,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        id: String,
        name: String,
        result: String,
    },

    #[serde(rename = "interrupt")]
    Interrupt { interrupts: Vec<InterruptInfo> },

    #[serde(rename = "turn_start")]
    TurnStart { turn: usize },

    #[serde(rename = "usage")]
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
    },

    #[serde(rename = "error")]
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },

    #[serde(rename = "done")]
    Done,

    #[serde(rename = "need_tool_execution")]
    NeedToolExecution {
        state: AgentState,
        tool_calls: Vec<ParsedToolCall>,
        completed_results: Vec<ToolCallOutcome>,
    },
}

// ── ProtocolError ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}
