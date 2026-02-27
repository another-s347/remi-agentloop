pub mod openai;

use crate::agent::Agent;
use crate::error::AgentError;
use crate::types::{ChatRequest, ChatResponseChunk};

/// Marker trait — any Agent with matching associated types is a ChatModel
pub trait ChatModel:
    Agent<Request = ChatRequest, Response = ChatResponseChunk, Error = AgentError>
{
}

impl<T> ChatModel for T where
    T: Agent<Request = ChatRequest, Response = ChatResponseChunk, Error = AgentError>
{
}
