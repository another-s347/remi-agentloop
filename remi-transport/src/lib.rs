// HTTP transport abstraction (HttpTransport trait + ReqwestTransport)
pub mod http;

// SSE encoding / decoding
pub mod sse;

// HTTP SSE client (connects to remote Agent)
#[cfg(feature = "http-client")]
pub mod http_client;

// HTTP SSE server (axum-based)
#[cfg(feature = "http-server")]
pub mod http_server;

// ── Re-exports ────────────────────────────────────────────────────────────────

pub use http::{HttpStreamingResponse, HttpTransport, HttpTransportError, MaybeSend};

#[cfg(feature = "http-client")]
pub use http::ReqwestTransport;

#[cfg(feature = "http-client")]
pub use http_client::HttpSseClient;

#[cfg(feature = "http-server")]
pub use http_server::HttpSseServer;
