//! # MCP Server
//!
//! Standalone implementation of the Model Context Protocol (MCP).
//!
//! MCP is a protocol that allows AI applications to connect to external
//! context providers (tools, resources, prompts) in a standardized way.
//!
//! ## Features
//!
//! - **Full MCP Protocol Support**: Implements the complete MCP specification
//! - **JSON-RPC 2.0 Based**: Standard request/response format
//! - **HTTP/HTTPS Transport**: Built on axum and reqwest
//! - **Type Safe**: Strongly typed Rust API
//! - **Zero Dependencies on remi-agentloop**: Completely standalone
//!
//! ## Quick Start
//!
//! ### Server
//!
//! ```ignore
//! use mcp_server::*;
//!
//! let server = McpServer::new("My MCP Server", "1.0.0")
//!     .with_tool(
//!         Tool {
//!             name: "calculate".into(),
//!             description: Some("Perform calculations".into()),
//!             input_schema: json!({
//!                 "type": "object",
//!                 "properties": {
//!                     "expression": { "type": "string" }
//!                 }
//!             }),
//!         },
//!         |args| async move {
//!             // Tool implementation
//!             Ok(CallToolResult {
//!                 content: vec![ToolContent::text("42")],
//!                 is_error: None,
//!             })
//!         },
//!     );
//!
//! server.serve("0.0.0.0:8080").await?;
//! ```
//!
//! ### Client
//!
//! ```ignore
//! use mcp_server::*;
//!
//! let client = McpClient::new("http://localhost:8080");
//! client.initialize("My Client", "1.0.0").await?;
//!
//! let tools = client.list_tools().await?;
//! let result = client.call_tool("calculate", Some(json!({
//!     "expression": "2 + 2"
//! }))).await?;
//! ```

pub mod protocol;
pub mod server;

#[cfg(feature = "client")]
pub mod client;

// Re-exports
pub use protocol::*;
pub use server::{McpServer, McpError};

#[cfg(feature = "client")]
pub use client::{McpClient, ClientError};

