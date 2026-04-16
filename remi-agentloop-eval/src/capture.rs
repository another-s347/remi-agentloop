use remi_core::tool::ToolDefinition;
use remi_core::types::{LoopInput, Message};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::EvalError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCapture {
    pub message: Message,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_tools: Vec<ToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl SessionCapture {
    pub fn new(message: Message) -> Self {
        Self {
            message,
            history: Vec::new(),
            extra_tools: Vec::new(),
            model: None,
            temperature: None,
            max_tokens: None,
            metadata: None,
        }
    }

    pub fn from_loop_input(input: &LoopInput) -> Result<Self, EvalError> {
        match input {
            LoopInput::Start {
                message,
                history,
                extra_tools,
                model,
                temperature,
                max_tokens,
                metadata,
            } => {
                if !matches!(message.role, remi_core::types::Role::User) {
                    return Err(EvalError::InvalidStartMessage);
                }

                Ok(Self {
                    message: message.clone(),
                    history: history.clone(),
                    extra_tools: extra_tools.clone(),
                    model: model.clone(),
                    temperature: *temperature,
                    max_tokens: *max_tokens,
                    metadata: metadata.clone(),
                })
            }
            LoopInput::Resume { .. } => Err(EvalError::MissingStartRequest),
        }
    }

    pub fn history(mut self, history: Vec<Message>) -> Self {
        self.history = history;
        self
    }

    pub fn extra_tools(mut self, extra_tools: Vec<ToolDefinition>) -> Self {
        self.extra_tools = extra_tools;
        self
    }

    pub fn metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}