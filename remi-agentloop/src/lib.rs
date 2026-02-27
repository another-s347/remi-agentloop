// ── Re-exports ────────────────────────────────────────────────────────────────

pub use remi_agentloop_macros::tool as tool_macro;

// Core
pub mod agent;
pub mod error;
pub mod types;
pub mod protocol;
pub mod config;
pub mod context;

// Model
pub mod model;

// Tool system
pub mod tool;

// Adapters
pub mod adapters;

// AgentLoop state machine
pub mod state;

// Builder
pub mod builder;

// Tracing
pub mod tracing;

// Transport
pub mod transport;

// ── Prelude ───────────────────────────────────────────────────────────────────

pub mod prelude {
    pub use crate::agent::{Agent, AgentExt, Layer};
    pub use crate::builder::{AgentBuilder, BuiltAgent};
    pub use crate::config::AgentConfig;
    pub use crate::context::{ContextStore, InMemoryStore, NoStore};
    pub use crate::error::AgentError;
    pub use crate::model::ChatModel;
    pub use crate::model::openai::OpenAIClient;
    pub use crate::protocol::{ProtocolAgent, ProtocolError, ProtocolEvent, ProtocolRequest};
    pub use crate::tool::{
        InterruptRequest, Tool, ToolDefinition, ToolOutput, ToolResult,
        registry::ToolRegistry,
    };
    pub use crate::tracing::{DynTracer, Tracer};
    pub use crate::tracing::stdout::StdoutTracer;
    pub use crate::types::{
        AgentEvent, ChatRequest, ChatResponseChunk, Content, ContentPart,
        InterruptId, InterruptInfo, Message, MessageId, ParsedToolCall,
        ResumePayload, Role, RunId, ThreadId, ToolCallResult,
    };
}
