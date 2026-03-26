//! Model Context Protocol (MCP) - Protocol Types
//!
//! MCP is a protocol for communication between AI applications and context providers.
//! This module defines the complete MCP wire format according to the specification.
//!
//! # Protocol Overview
//!
//! MCP uses JSON-RPC 2.0 as its base protocol with extensions for:
//! - Resources (files, documents, data sources)
//! - Tools (executable functions)
//! - Prompts (reusable templates)
//! - Sampling (LLM completion requests)
//!
//! # Message Flow
//!
//! ```text
//! Client                           Server
//!   |                                |
//!   |-- initialize ----------------->|
//!   |<-- initialize result ----------|
//!   |                                |
//!   |-- tools/list ----------------->|
//!   |<-- tool list ------------------|
//!   |                                |
//!   |-- tools/call ----------------->|
//!   |<-- tool result ----------------|
//!   |                                |
//!   |-- resources/list ------------->|
//!   |<-- resource list --------------|
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── JSON-RPC 2.0 Base ─────────────────────────────────────────────────────────

/// JSON-RPC 2.0 request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: RequestId,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 response (success)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: RequestId,
    pub result: serde_json::Value,
}

/// JSON-RPC 2.0 error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub jsonrpc: String,
    pub id: RequestId,
    pub error: ErrorObject,
}

/// JSON-RPC error object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorObject {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// JSON-RPC notification (no response expected)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// Request ID (string or number)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Number(i64),
}

impl RequestId {
    pub fn new_string(s: impl Into<String>) -> Self {
        RequestId::String(s.into())
    }

    pub fn new_number(n: i64) -> Self {
        RequestId::Number(n)
    }
}

// ── MCP Protocol Messages ─────────────────────────────────────────────────────

/// MCP protocol version
pub const MCP_VERSION: &str = "2024-11-05";

// ── Initialize ─────────────────────────────────────────────────────────────────

/// Client capabilities
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClientCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<serde_json::Value>,
}

/// Server capabilities
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptsCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribe: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// Initialize request parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    pub client_info: Implementation,
}

/// Initialize response result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: Implementation,
}

/// Implementation info (client or server)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Implementation {
    pub name: String,
    pub version: String,
}

// ── Resources ──────────────────────────────────────────────────────────────────

/// Resource definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Resource content (text or blob)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ResourceContent {
    Text {
        uri: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        text: String,
    },
    Blob {
        uri: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        blob: String, // base64-encoded
    },
}

/// List resources request parameters
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListResourcesParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// List resources result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResourcesResult {
    pub resources: Vec<Resource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Read resource request parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResourceParams {
    pub uri: String,
}

/// Read resource result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResourceResult {
    pub contents: Vec<ResourceContent>,
}

// ── Tools ──────────────────────────────────────────────────────────────────────

/// Tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: serde_json::Value, // JSON Schema
}

/// List tools request parameters
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListToolsParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// List tools result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsResult {
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Call tool request parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolParams {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}

/// Call tool result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    pub content: Vec<ToolContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// Tool result content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolContent {
    Text {
        text: String,
    },
    Image {
        data: String, // base64
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    Resource {
        uri: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "mimeType")]
        mime_type: Option<String>,
        text: String,
    },
}

impl ToolContent {
    pub fn text(s: impl Into<String>) -> Self {
        ToolContent::Text { text: s.into() }
    }

    pub fn error(s: impl Into<String>) -> Self {
        ToolContent::Text { text: format!("Error: {}", s.into()) }
    }
}

// ── Prompts ────────────────────────────────────────────────────────────────────

/// Prompt definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<PromptArgument>>,
}

/// Prompt argument definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// List prompts request parameters
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListPromptsParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// List prompts result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPromptsResult {
    pub prompts: Vec<Prompt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Get prompt request parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPromptParams {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<HashMap<String, String>>,
}

/// Get prompt result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPromptResult {
    pub messages: Vec<PromptMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Prompt message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMessage {
    pub role: MessageRole,
    pub content: MessageContent,
}

/// Message role
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
}

/// Message content (text or image)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MessageContent {
    Text {
        text: String,
    },
    Image {
        data: String, // base64
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

// ── Sampling ───────────────────────────────────────────────────────────────────

/// Sampling request (client asks server to use LLM)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateMessageParams {
    pub messages: Vec<SamplingMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_preferences: Option<ModelPreferences>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Sampling message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplingMessage {
    pub role: MessageRole,
    pub content: SamplingContent,
}

/// Sampling content (text or image)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SamplingContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

/// Content part for sampling
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentPart {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

/// Model preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPreferences {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hints: Option<Vec<ModelHint>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_priority: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_priority: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intelligence_priority: Option<f32>,
}

/// Model hint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelHint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Create message result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateMessageResult {
    pub role: MessageRole,
    pub content: SamplingContent,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

// ── Logging ────────────────────────────────────────────────────────────────────

/// Log level
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Notice,
    Warning,
    Error,
    Critical,
    Alert,
    Emergency,
}

/// Logging notification parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingMessageParams {
    pub level: LogLevel,
    pub logger: String,
    pub data: serde_json::Value,
}

// ── Pagination ─────────────────────────────────────────────────────────────────

/// Pagination cursor (opaque string)
pub type Cursor = String;

// ── Error Codes ────────────────────────────────────────────────────────────────

/// Standard JSON-RPC error codes
pub mod error_codes {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
}

/// MCP-specific error codes
pub mod mcp_error_codes {
    pub const RESOURCE_NOT_FOUND: i32 = -32001;
    pub const TOOL_NOT_FOUND: i32 = -32002;
    pub const PROMPT_NOT_FOUND: i32 = -32003;
    pub const INVALID_TOOL_INPUT: i32 = -32004;
}

// ── Helper Functions ───────────────────────────────────────────────────────────

impl JsonRpcRequest {
    pub fn new(id: RequestId, method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params,
        }
    }
}

impl JsonRpcResponse {
    pub fn new(id: RequestId, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result,
        }
    }
}

impl JsonRpcError {
    pub fn new(id: RequestId, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            error: ErrorObject {
                code,
                message: message.into(),
                data: None,
            },
        }
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.error.data = Some(data);
        self
    }
}

impl JsonRpcNotification {
    pub fn new(method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_serialization() {
        let req = JsonRpcRequest::new(
            RequestId::new_string("1"),
            "tools/list",
            None,
        );
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("2.0"));
        assert!(json.contains("tools/list"));
    }

    #[test]
    fn test_response_serialization() {
        let resp = JsonRpcResponse::new(
            RequestId::new_number(1),
            serde_json::json!({"tools": []}),
        );
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("result"));
    }

    #[test]
    fn test_error_serialization() {
        let err = JsonRpcError::new(
            RequestId::new_string("1"),
            error_codes::METHOD_NOT_FOUND,
            "Method not found",
        );
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("error"));
        assert!(json.contains("-32601"));
    }
}
