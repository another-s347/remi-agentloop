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
    pub fn new() -> Self { Self(uuid_v4()) }
}
impl RunId {
    pub fn new() -> Self { Self(uuid_v4()) }
}
impl MessageId {
    pub fn new() -> Self { Self(uuid_v4()) }
}
impl InterruptId {
    pub fn new() -> Self { Self(uuid_v4()) }
}

impl Default for ThreadId { fn default() -> Self { Self::new() } }
impl Default for RunId    { fn default() -> Self { Self::new() } }
impl Default for MessageId { fn default() -> Self { Self::new() } }
impl Default for InterruptId { fn default() -> Self { Self::new() } }

impl fmt::Display for ThreadId  { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str(&self.0) } }
impl fmt::Display for RunId     { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str(&self.0) } }
impl fmt::Display for MessageId { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str(&self.0) } }
impl fmt::Display for InterruptId { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str(&self.0) } }

// ── Multimodal Content ────────────────────────────────────────────────────────

/// 消息内容——兼容 OpenAI content 字段
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl Content {
    pub fn text(s: impl Into<String>) -> Self { Content::Text(s.into()) }
    pub fn parts(parts: Vec<ContentPart>) -> Self { Content::Parts(parts) }

    pub fn text_content(&self) -> String {
        match self {
            Content::Text(s) => s.clone(),
            Content::Parts(parts) => parts.iter()
                .filter_map(|p| match p { ContentPart::Text { text } => Some(text.as_str()), _ => None })
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
            image_url: ImageUrlDetail { url: url.into(), detail: None },
        }
    }
    pub fn image_base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        ContentPart::ImageBase64 { media_type: media_type.into(), data: data.into() }
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
    pub id: MessageId,
    pub role: Role,
    pub content: Content,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::User,
            content: Content::text(text),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::System,
            content: Content::text(text),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::Assistant,
            content: Content::text(text),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant_with_tool_calls(text: impl Into<String>, tool_calls: Vec<ToolCallMessage>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::Assistant,
            content: Content::text(text),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, result: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::Tool,
            content: Content::text(result),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    pub fn user_multimodal(parts: Vec<ContentPart>) -> Self {
        Self {
            id: MessageId::new(),
            role: Role::User,
            content: Content::parts(parts),
            tool_calls: None,
            tool_call_id: None,
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
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub enum ChatResponseChunk {
    Delta { content: String, role: Option<Role> },
    ToolCallStart { index: usize, id: String, name: String },
    ToolCallDelta { index: usize, arguments_delta: String },
    Usage { prompt_tokens: u32, completion_tokens: u32, total_tokens: u32 },
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
    ToolCallStart { id: String, name: String },
    ToolCallArgumentsDelta { id: String, delta: String },
    ToolDelta { id: String, name: String, delta: String },
    ToolResult { id: String, name: String, result: String },
    Interrupt {
        interrupts: Vec<InterruptInfo>,
    },
    TurnStart { turn: usize },
    Usage { prompt_tokens: u32, completion_tokens: u32 },
    Done,
    Error(AgentError),
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
#[derive(Debug, Clone)]
pub struct ParsedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Single tool call execution result
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub id: String,
    pub name: String,
    pub result: String,
}
