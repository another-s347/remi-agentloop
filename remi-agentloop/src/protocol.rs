use crate::types::*;
use crate::tool::ToolDefinition;
use serde::{Deserialize, Serialize};

/// 标准协议请求——JSON 可序列化
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,

    pub messages: Vec<Message>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for ProtocolRequest {
    fn default() -> Self {
        Self {
            thread_id: None,
            messages: vec![],
            tools: None,
            model: None,
            temperature: None,
            max_tokens: None,
            metadata: None,
            extra: Default::default(),
        }
    }
}

/// 标准协议流式响应事件——JSON 可序列化
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
    ToolDelta { id: String, name: String, delta: String },

    #[serde(rename = "tool_result")]
    ToolResult { id: String, name: String, result: String },

    #[serde(rename = "interrupt")]
    Interrupt { interrupts: Vec<InterruptInfo> },

    #[serde(rename = "turn_start")]
    TurnStart { turn: usize },

    #[serde(rename = "usage")]
    Usage { prompt_tokens: u32, completion_tokens: u32 },

    #[serde(rename = "error")]
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },

    #[serde(rename = "done")]
    Done,
}

/// 标准协议错误
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

impl From<crate::error::AgentError> for ProtocolError {
    fn from(e: crate::error::AgentError) -> Self {
        ProtocolError {
            code: "agent_error".into(),
            message: e.to_string(),
        }
    }
}

/// 符合标准协议的 Agent (marker)
pub trait ProtocolAgent:
    crate::agent::Agent<Request = ProtocolRequest, Response = ProtocolEvent, Error = ProtocolError>
{
}

impl<T> ProtocolAgent for T where
    T: crate::agent::Agent<Request = ProtocolRequest, Response = ProtocolEvent, Error = ProtocolError>
{
}

// ── Conversions ───────────────────────────────────────────────────────────────

impl From<AgentEvent> for ProtocolEvent {
    fn from(e: AgentEvent) -> Self {
        match e {
            AgentEvent::RunStart { thread_id, run_id, metadata } => ProtocolEvent::RunStart {
                thread_id: thread_id.to_string(),
                run_id: run_id.to_string(),
                metadata,
            },
            AgentEvent::TextDelta(s) => ProtocolEvent::Delta { content: s, role: None },
            AgentEvent::ToolCallStart { id, name } => ProtocolEvent::ToolCallStart { id, name },
            AgentEvent::ToolCallArgumentsDelta { id, delta } => ProtocolEvent::ToolCallDelta { id, arguments_delta: delta },
            AgentEvent::ToolDelta { id, name, delta } => ProtocolEvent::ToolDelta { id, name, delta },
            AgentEvent::ToolResult { id, name, result } => ProtocolEvent::ToolResult { id, name, result },
            AgentEvent::Interrupt { interrupts } => ProtocolEvent::Interrupt { interrupts },
            AgentEvent::TurnStart { turn } => ProtocolEvent::TurnStart { turn },
            AgentEvent::Usage { prompt_tokens, completion_tokens } => ProtocolEvent::Usage { prompt_tokens, completion_tokens },
            AgentEvent::Done => ProtocolEvent::Done,
            AgentEvent::Error(e) => ProtocolEvent::Error { message: e.to_string(), code: None },
        }
    }
}

impl From<ProtocolRequest> for (Vec<Message>, Option<String>) {
    fn from(r: ProtocolRequest) -> Self {
        (r.messages, r.model)
    }
}
