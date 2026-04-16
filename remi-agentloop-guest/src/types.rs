//! Serde-compatible type definitions for the remi-agentloop WASM guest interface.
//!
//! These types mirror the host-side types in `remi-agentloop` and are
//! serialization-compatible via JSON. They are intentionally kept free of
//! async / platform-specific dependencies so they compile to `wasm32`.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn synthetic_id(prefix: &str) -> String {
    format!("{prefix}-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

// ── Identifiers ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThreadId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InterruptId(pub String);

impl MessageId {
    pub fn new() -> Self {
        Self(synthetic_id("msg"))
    }
}

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
    /// Optional user identifier for `Role::User` messages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Chain-of-thought / reasoning text returned by thinking models.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// User-defined metadata attached to this message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::User,
            content: Content::text(text),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            metadata: None,
        }
    }

    pub fn user_content(content: Content) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::User,
            content,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            metadata: None,
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::System,
            content: Content::text(text),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            metadata: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::Assistant,
            content: Content::text(text),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            metadata: None,
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
            name: None,
            reasoning_content,
            metadata: None,
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_name_opt(mut self, name: Option<String>) -> Self {
        if let Some(name) = name {
            self.name = Some(name);
        }
        self
    }

    pub fn with_metadata(mut self, metadata: impl Into<serde_json::Value>) -> Self {
        self.metadata = Some(metadata.into());
        self
    }

    pub fn tool_result(tool_call_id: impl Into<String>, result: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::Tool,
            content: Content::text(result),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
            reasoning_content: None,
            metadata: None,
        }
    }

    pub fn tool_result_content(tool_call_id: impl Into<String>, content: Content) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::Tool,
            content,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
            reasoning_content: None,
            metadata: None,
        }
    }

    pub fn user_multimodal(parts: Vec<ContentPart>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::User,
            content: Content::parts(parts),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            metadata: None,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_prompt: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_retry: Option<RateLimitRetryPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RateLimitRetryPolicy {
    pub max_retries: usize,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub multiplier: f64,
    pub respect_retry_after: bool,
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
        content: Content,
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
        message: Message,
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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_interrupts: Vec<InterruptInfo>,
        results: Vec<ToolCallOutcome>,
    },
}

impl LoopInput {
    pub fn start(msg: impl Into<String>) -> Self {
        Self::Start {
            message: Message::user(msg),
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
            message: Message::user_content(content),
            history: vec![],
            extra_tools: vec![],
            model: None,
            temperature: None,
            max_tokens: None,
            metadata: None,
        }
    }

    pub fn start_message(message: Message) -> Self {
        Self::Start {
            message,
            history: vec![],
            extra_tools: vec![],
            model: None,
            temperature: None,
            max_tokens: None,
            metadata: None,
        }
    }

    pub fn resume(
        state: AgentState,
        pending_interrupts: Vec<InterruptInfo>,
        results: Vec<ToolCallOutcome>,
    ) -> Self {
        Self::Resume {
            state,
            pending_interrupts,
            results,
        }
    }

    pub fn history(mut self, msgs: Vec<Message>) -> Self {
        if let Self::Start { history, .. } = &mut self {
            *history = msgs;
        }
        self
    }

    pub fn extra_tools(mut self, defs: Vec<ToolDefinition>) -> Self {
        if let Self::Start { extra_tools, .. } = &mut self {
            *extra_tools = defs;
        }
        self
    }

    pub fn model(mut self, model_name: impl Into<String>) -> Self {
        if let Self::Start { model, .. } = &mut self {
            *model = Some(model_name.into());
        }
        self
    }

    pub fn temperature(mut self, value: f64) -> Self {
        if let Self::Start { temperature, .. } = &mut self {
            *temperature = Some(value);
        }
        self
    }

    pub fn max_tokens(mut self, value: u32) -> Self {
        if let Self::Start { max_tokens, .. } = &mut self {
            *max_tokens = Some(value);
        }
        self
    }

    pub fn metadata(mut self, value: serde_json::Value) -> Self {
        if let Self::Start { metadata, .. } = &mut self {
            *metadata = Some(value);
        }
        self
    }

    pub fn message_metadata(mut self, value: serde_json::Value) -> Self {
        if let Self::Start { message, .. } = &mut self {
            message.metadata = Some(value);
        }
        self
    }

    pub fn user_name(mut self, name: impl Into<String>) -> Self {
        if let Self::Start { message, .. } = &mut self {
            message.name = Some(name.into());
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

    /// Arbitrary user-defined protocol event.  The `event_type` field carries
    /// the custom sub-type name; `extra` holds any additional JSON payload.
    #[serde(rename = "custom")]
    Custom {
        event_type: String,
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        extra: serde_json::Value,
    },
}

// ── CustomProtocolEvent trait ────────────────────────────────────────────────

/// Trait for types that can be losslessly round-tripped through
/// [`ProtocolEvent::Custom`].  Implement this on any struct / enum that you
/// want to embed in the standard event stream.
pub trait CustomProtocolEvent: Sized + serde::Serialize + serde::de::DeserializeOwned {
    /// Unique string tag that identifies this custom event type.
    const EVENT_TYPE: &'static str;

    /// Wrap `self` into a [`ProtocolEvent::Custom`].
    fn to_protocol_event(&self) -> Result<ProtocolEvent, serde_json::Error> {
        Ok(ProtocolEvent::Custom {
            event_type: Self::EVENT_TYPE.to_owned(),
            extra: serde_json::to_value(self)?,
        })
    }

    /// Try to extract `Self` from a [`ProtocolEvent`].  Returns `None` when
    /// the event is not a `Custom` event or the `event_type` tag does not
    /// match; returns `Some(Err(_))` if deserialization fails.
    fn from_protocol_event(event: &ProtocolEvent) -> Option<Result<Self, serde_json::Error>> {
        if let ProtocolEvent::Custom { event_type, extra } = event {
            if event_type == Self::EVENT_TYPE {
                return Some(serde_json::from_value(extra.clone()));
            }
        }
        None
    }
}

/// Convert any [`CustomProtocolEvent`] into a [`ProtocolEvent`] via the
/// `Custom` variant.  Panics if serialization fails (use
/// [`CustomProtocolEvent::to_protocol_event`] for a fallible version).
impl<T: CustomProtocolEvent> From<T> for ProtocolEvent {
    fn from(value: T) -> Self {
        value
            .to_protocol_event()
            .expect("CustomProtocolEvent serialization failed")
    }
}

// ── ProtocolError ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

// ── AgentConfig (guest-side) ─────────────────────────────────────────────────

/// Agent runtime configuration as seen by the guest.
///
/// Obtained by calling [`get_config()`][crate::get_config] which pulls from
/// the host via the imported `remi:agentloop/config` WIT interface.
///
/// This mirrors `remi_core::config::AgentConfig` from the host crate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_retry: Option<RateLimitRetryPolicy>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub headers: std::collections::HashMap<String, String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub extra: serde_json::Value,
}

impl AgentConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge: fields from `other` override `self` when `Some`.
    pub fn merge(mut self, other: &AgentConfig) -> Self {
        if other.api_key.is_some() {
            self.api_key = other.api_key.clone();
        }
        if other.model.is_some() {
            self.model = other.model.clone();
        }
        if other.base_url.is_some() {
            self.base_url = other.base_url.clone();
        }
        if other.temperature.is_some() {
            self.temperature = other.temperature;
        }
        if other.max_tokens.is_some() {
            self.max_tokens = other.max_tokens;
        }
        if other.timeout_ms.is_some() {
            self.timeout_ms = other.timeout_ms;
        }
        if other.rate_limit_retry.is_some() {
            self.rate_limit_retry = other.rate_limit_retry.clone();
        }
        for (k, v) in &other.headers {
            self.headers.insert(k.clone(), v.clone());
        }
        if !other.extra.is_null() {
            self.extra = other.extra.clone();
        }
        self
    }
}

// ── ApiVersion ───────────────────────────────────────────────────────────────

/// Semantic version triple used for host/guest compatibility negotiation.
///
/// The runner enforces:
/// - `guest.api_version.major == HOST_API_VERSION.major`
/// - `guest.min_host_version <= HOST_API_VERSION`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ApiVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl ApiVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl std::fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}
