// ── Re-exports ────────────────────────────────────────────────────────────────

pub use remi_agentloop_macros::tool as tool_macro;

// Core
pub mod agent;
pub mod checkpoint;
pub mod config;
pub mod context;
pub mod error;
pub mod interrupt;
pub mod protocol;
pub mod types;
pub mod union;

// Model trait
pub mod model;

// Tool trait + registry
pub mod tool;

// Adapters
pub mod adapters;

// Step function & AgentState
pub mod state;

// Agent loop (composable step + tool execution core)
pub mod agent_loop;

// Builder
pub mod builder;

// Tracing
pub mod tracing;

// ── Prelude ───────────────────────────────────────────────────────────────────

pub mod prelude {
    pub use crate::agent::{Agent, AgentExt, Layer};
    pub use crate::agent_loop::AgentLoop;
    pub use crate::builder::{AgentBuilder, BuiltAgent};
    pub use crate::checkpoint::{
        Checkpoint, CheckpointId, CheckpointStatus, CheckpointStore, InMemoryCheckpointStore,
        NoCheckpointStore,
    };
    pub use crate::config::{AgentConfig, ConfigProvider};
    pub use crate::context::{ContextStore, ContextStoreExt, InMemoryStore, NoStore};
    pub use crate::error::AgentError;
    pub use crate::interrupt::{InterruptHandler, InterruptRouter};
    pub use crate::model::ChatModel;
    pub use crate::protocol::{ProtocolAgent, ProtocolError, ProtocolEvent};
    pub use crate::state::{step, Action, AgentPhase, AgentState, StepConfig, StepEvent};
    pub use crate::tool::{
        registry::{DefaultToolRegistry, ToolRegistry},
        InterruptRequest, Tool, ToolContext, ToolDefinition, ToolOutput, ToolResult,
    };
    pub use crate::tracing::stdout::StdoutTracer;
    pub use crate::tracing::{DynTracer, Tracer};
    pub use crate::types::{
        AgentEvent, ChatInput, ChatRequest, ChatResponseChunk, Content, ContentPart, InterruptId,
        InterruptInfo, LoopInput, Message, MessageId, ParsedToolCall, ResumePayload, Role, RunId,
        ThreadId, ToolCallOutcome, ToolCallResult,
    };
    pub use crate::union::{Union2, Union3, Union4};
}
