//! Facade crate — re-exports everything from the remi sub-crates.
//!
//! Users can depend on `remi-agentloop` to get the full framework,
//! or depend on individual sub-crates (`remi-core`, `remi-model`,
//! `remi-tool`, `remi-transport`) for finer-grained control.

// ── Re-exports from remi-core ─────────────────────────────────────────────────

pub use remi_agentloop_macros::tool as tool_macro;

pub use remi_core::{
    agent, adapters, agent_loop, builder, checkpoint, config, context,
    error, interrupt, model, protocol, state, tool, tracing, types, union,
};

// ── Re-exports from remi-transport ────────────────────────────────────────────

/// HTTP transport abstraction (HttpTransport trait, ReqwestTransport, SSE)
pub mod transport {
    pub use remi_transport::*;
}

/// HTTP transport abstraction — re-exported module
pub mod http {
    pub use remi_transport::http::*;
}

// ── Re-exports from remi-model ────────────────────────────────────────────────

/// OpenAI-compatible model implementations
pub mod openai {
    pub use remi_model::openai::*;
}

// ── Prelude ───────────────────────────────────────────────────────────────────

pub mod prelude {
    // Core
    pub use remi_core::prelude::*;

    // Transport
    pub use remi_transport::HttpTransport;
    #[cfg(feature = "http-client")]
    pub use remi_transport::ReqwestTransport;

    // Model
    pub use remi_model::OpenAIClient;

    // Tool implementations
    #[cfg(feature = "tool-bash")]
    pub use remi_tool::BashTool;
    #[cfg(feature = "tool-fs")]
    pub use remi_tool::FsTool;
    #[cfg(feature = "tool-fs-virtual")]
    pub use remi_tool::VirtualFsTool;
    #[cfg(feature = "tool-bash-virtual")]
    pub use remi_tool::VirtualBashTool;
}
