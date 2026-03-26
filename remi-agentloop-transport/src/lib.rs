// HTTP transport abstraction (HttpTransport trait + ReqwestTransport)
pub mod http;

// SSE encoding / decoding
pub mod sse;

// HTTP SSE client — transport-generic, works on native/WASM/browser
pub mod http_client;

// HTTP SSE server (axum-based)
#[cfg(feature = "http-server")]
pub mod http_server;

// ACP (Agent Communication Protocol) — standalone protocol for agent-to-agent communication
pub mod acp;

// ── Re-exports ────────────────────────────────────────────────────────────────

pub use http::{HttpStreamingResponse, HttpTransport, HttpTransportError, MaybeSend};

#[cfg(feature = "http-client")]
pub use http::ReqwestTransport;

pub use http_client::HttpSseClient;

#[cfg(feature = "http-server")]
pub use http_server::HttpSseServer;

// ACP exports
pub use acp::{
    AcpAgent, AcpClient, AcpContent, AcpContentPart, AcpError, AcpEvent, AcpMessage, AcpRequest,
    AcpRouter, AcpToolDefinition, AgentCapabilities, AgentCost, AgentId, AgentPerformance,
    AgentQueryRequest, AgentRegistry, DelegationId, ExecutionConstraints, MessageRole,
    RoutingHints, SessionId, TaskId, TaskStatus, ToolParameter, TraceLevel,
};

#[cfg(feature = "http-server")]
pub use acp::AcpServer;
