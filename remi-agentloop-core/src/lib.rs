//! # remi-agentloop-core
//!
//! Core framework crate for `remi-agentloop` — provides all the fundamental
//! types, traits, and infrastructure needed to build LLM-powered agent loops.
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │              BuiltAgent                      │  ← persistent memory + run lifecycle
//! │  AgentBuilder  ──builds──►  AgentLoop        │  ← step + tool execution loop
//! │                                step()        │  ← single model call (pure function)
//! └──────────────────────────────────────────────┘
//!        │ implements
//!        ▼
//!    Agent<Request=LoopInput, Response=AgentEvent>
//! ```
//!
//! ## Quick Start
//!
//! ```ignore
//! use remi_agentloop_core::prelude::*;
//!
//! // 1. Build an agent (with OpenAI-compatible model)
//! let agent = AgentBuilder::new()
//!     .model(my_model)  // any ChatModel impl
//!     .system("You are a helpful assistant.")
//!     .max_turns(10)
//!     .build();         // → BuiltAgent
//!
//! // 2. Start a conversation thread
//! let thread_id = agent.create_thread().await?;
//!
//! // 3. Chat — stream AgentEvents
//! let mut stream = agent.chat_in_thread(
//!     &thread_id,
//!     ChatInput::text("Hello!"),
//! ).await?;
//!
//! while let Some(event) = stream.next().await {
//!     match event {
//!         AgentEvent::TextDelta(s) => print!("{s}"),
//!         AgentEvent::Done => break,
//!         AgentEvent::Error(e) => return Err(e.into()),
//!         _ => {}
//!     }
//! }
//! ```
//!
//! ## Crate modules
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`agent`] | Core [`Agent`](agent::Agent) trait and [`AgentExt`](agent::AgentExt) combinators |
//! | [`builder`] | [`AgentBuilder`](builder::AgentBuilder) typestate builder → [`BuiltAgent`](builder::BuiltAgent) |
//! | [`agent_loop`] | [`AgentLoop`](agent_loop::AgentLoop) — composable step + tool execution engine |
//! | [`state`] | [`step()`](state::step) primitive, [`AgentState`](state::AgentState), [`Action`](state::Action) |
//! | [`tool`] | [`Tool`](tool::Tool) trait, [`ToolOutput`](tool::ToolOutput), [`ToolResult`](tool::ToolResult), registry |
//! | [`types`] | [`Message`](types::Message), [`Content`](types::Content), [`AgentEvent`](types::AgentEvent), identifiers |
//! | [`config`] | [`AgentConfig`](config::AgentConfig) — runtime configuration |
//! | [`context`] | [`ContextStore`](context::ContextStore) — conversation persistence |
//! | [`checkpoint`] | [`CheckpointStore`](checkpoint::CheckpointStore) — durable execution snapshots |
//! | [`model`] | [`ChatModel`](model::ChatModel) marker trait |
//! | [`adapters`] | [`Layer`](agent::Layer) implementations (logging, retry, …) |
//! | [`tracing`] | [`Tracer`](tracing::Tracer) trait + [`StdoutTracer`](tracing::stdout::StdoutTracer) |
//! | [`protocol`] | [`ProtocolAgent`](protocol::ProtocolAgent) — SSE-over-HTTP transport protocol |

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
    pub use crate::protocol::{CustomProtocolEvent, ProtocolAgent, ProtocolError, ProtocolEvent};
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
