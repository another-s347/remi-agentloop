use crate::types::{InterruptId, RunId, ThreadId};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[cfg(feature = "http-client")]
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("SSE parse error: {message}")]
    SseParse { message: String },

    #[error("Tool execution error [{tool_name}]: {message}")]
    ToolExecution { tool_name: String, message: String },

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Model error: {0}")]
    Model(String),

    #[error("Max turns ({max}) exceeded")]
    MaxTurnsExceeded { max: usize },

    #[error("Thread not found: {0}")]
    ThreadNotFound(ThreadId),

    #[error("Run not found: {0}")]
    RunNotFound(RunId),

    #[error("Interrupt not found: {0}")]
    InterruptNotFound(InterruptId),

    #[error("Resume incomplete: expected {expected} interrupt results, got {got}")]
    ResumeIncomplete { expected: usize, got: usize },

    #[error("Context store error: {0}")]
    Store(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("{0}")]
    Other(String),
}

impl Clone for AgentError {
    fn clone(&self) -> Self {
        match self {
            #[cfg(feature = "http-client")]
            Self::Http(e) => Self::Other(e.to_string()),
            Self::Json(e) => Self::Other(e.to_string()),
            Self::SseParse { message } => Self::SseParse { message: message.clone() },
            Self::ToolExecution { tool_name, message } => Self::ToolExecution {
                tool_name: tool_name.clone(),
                message: message.clone(),
            },
            Self::ToolNotFound(s) => Self::ToolNotFound(s.clone()),
            Self::Model(s) => Self::Model(s.clone()),
            Self::MaxTurnsExceeded { max } => Self::MaxTurnsExceeded { max: *max },
            Self::ThreadNotFound(id) => Self::ThreadNotFound(id.clone()),
            Self::RunNotFound(id) => Self::RunNotFound(id.clone()),
            Self::InterruptNotFound(id) => Self::InterruptNotFound(id.clone()),
            Self::ResumeIncomplete { expected, got } => Self::ResumeIncomplete { expected: *expected, got: *got },
            Self::Store(s) => Self::Store(s.clone()),
            Self::Io(s) => Self::Io(s.clone()),
            Self::Other(s) => Self::Other(s.clone()),
        }
    }
}

impl AgentError {
    pub fn sse_parse(msg: impl Into<String>) -> Self {
        Self::SseParse { message: msg.into() }
    }

    pub fn tool(tool_name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ToolExecution {
            tool_name: tool_name.into(),
            message: message.into(),
        }
    }

    pub fn model(msg: impl Into<String>) -> Self {
        Self::Model(msg.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}
