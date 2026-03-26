//! MCP Server Implementation
//!
//! Provides a full MCP server with support for:
//! - Resources (data sources)
//! - Tools (executable functions)
//! - Prompts (templates)
//! - Logging
//!
//! # Example
//!
//! ```ignore
//! let server = McpServer::new("My Server", "1.0.0")
//!     .with_tool(Tool {
//!         name: "calculate".into(),
//!         description: Some("Do math".into()),
//!         input_schema: json!({ ... }),
//!     }, |args| async move {
//!         Ok(CallToolResult {
//!             content: vec![ToolContent::text("42")],
//!             is_error: None,
//!         })
//!     })
//!     .with_resource(Resource {
//!         uri: "file:///data.txt".into(),
//!         name: "data".into(),
//!         ...
//!     }, |uri| async move {
//!         Ok(ReadResourceResult {
//!             contents: vec![...],
//!         })
//!     });
//!
//! server.serve("0.0.0.0:8080").await?;
//! ```

use crate::protocol::*;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;

// ── Error Types ───────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("Method not found: {0}")]
    MethodNotFound(String),
    
    #[error("Invalid params: {0}")]
    InvalidParams(String),
    
    #[error("Resource not found: {0}")]
    ResourceNotFound(String),
    
    #[error("Tool not found: {0}")]
    ToolNotFound(String),
    
    #[error("Prompt not found: {0}")]
    PromptNotFound(String),
    
    #[error("Internal error: {0}")]
    Internal(String),
    
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl McpError {
    pub fn to_json_rpc_error(&self, id: RequestId) -> JsonRpcError {
        let (code, message) = match self {
            McpError::MethodNotFound(m) => (error_codes::METHOD_NOT_FOUND, m.clone()),
            McpError::InvalidParams(m) => (error_codes::INVALID_PARAMS, m.clone()),
            McpError::ResourceNotFound(m) => (mcp_error_codes::RESOURCE_NOT_FOUND, m.clone()),
            McpError::ToolNotFound(m) => (mcp_error_codes::TOOL_NOT_FOUND, m.clone()),
            McpError::PromptNotFound(m) => (mcp_error_codes::PROMPT_NOT_FOUND, m.clone()),
            McpError::Internal(m) => (error_codes::INTERNAL_ERROR, m.clone()),
            McpError::Serialization(e) => (error_codes::INTERNAL_ERROR, e.to_string()),
        };
        JsonRpcError::new(id, code, message)
    }
}

// ── Handler Types ─────────────────────────────────────────────────────────────

/// Tool handler function
pub type ToolHandler = Arc<
    dyn Fn(serde_json::Value) -> Pin<Box<dyn Future<Output = Result<CallToolResult, McpError>> + Send>>
        + Send
        + Sync,
>;

/// Resource handler function
pub type ResourceHandler = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<ReadResourceResult, McpError>> + Send>>
        + Send
        + Sync,
>;

/// Prompt handler function
pub type PromptHandler = Arc<
    dyn Fn(HashMap<String, String>) -> Pin<Box<dyn Future<Output = Result<GetPromptResult, McpError>> + Send>>
        + Send
        + Sync,
>;

// ── MCP Server ────────────────────────────────────────────────────────────────

/// MCP Server
pub struct McpServer {
    server_info: Implementation,
    capabilities: ServerCapabilities,
    
    tools: Arc<RwLock<HashMap<String, (Tool, ToolHandler)>>>,
    resources: Arc<RwLock<HashMap<String, (Resource, ResourceHandler)>>>,
    prompts: Arc<RwLock<HashMap<String, (Prompt, PromptHandler)>>>,
    
    initialized: Arc<RwLock<bool>>,
}

impl McpServer {
    /// Create a new MCP server
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            server_info: Implementation {
                name: name.into(),
                version: version.into(),
            },
            capabilities: ServerCapabilities {
                experimental: None,
                logging: Some(serde_json::json!({})),
                prompts: Some(PromptsCapability {
                    list_changed: Some(false),
                }),
                resources: Some(ResourcesCapability {
                    subscribe: Some(false),
                    list_changed: Some(false),
                }),
                tools: Some(ToolsCapability {
                    list_changed: Some(false),
                }),
            },
            tools: Arc::new(RwLock::new(HashMap::new())),
            resources: Arc::new(RwLock::new(HashMap::new())),
            prompts: Arc::new(RwLock::new(HashMap::new())),
            initialized: Arc::new(RwLock::new(false)),
        }
    }

    /// Register a tool with a handler (non-async for builder pattern)
    pub fn with_tool<F, Fut>(mut self, tool: Tool, handler: F) -> Self
    where
        F: Fn(serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<CallToolResult, McpError>> + Send + 'static,
    {
        let name = tool.name.clone();
        let handler: ToolHandler = Arc::new(move |args| Box::pin(handler(args)));
        
        // Get mutable access to the Arc - this is safe during construction
        let tools = Arc::get_mut(&mut self.tools).unwrap().get_mut();
        tools.insert(name, (tool, handler));
        
        self
    }

    /// Register a resource with a handler (non-async for builder pattern)
    pub fn with_resource<F, Fut>(mut self, resource: Resource, handler: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<ReadResourceResult, McpError>> + Send + 'static,
    {
        let uri = resource.uri.clone();
        let handler: ResourceHandler = Arc::new(move |uri| Box::pin(handler(uri)));
        
        let resources = Arc::get_mut(&mut self.resources).unwrap().get_mut();
        resources.insert(uri, (resource, handler));
        
        self
    }

    /// Register a prompt with a handler (non-async for builder pattern)
    pub fn with_prompt<F, Fut>(mut self, prompt: Prompt, handler: F) -> Self
    where
        F: Fn(HashMap<String, String>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<GetPromptResult, McpError>> + Send + 'static,
    {
        let name = prompt.name.clone();
        let handler: PromptHandler = Arc::new(move |args| Box::pin(handler(args)));
        
        let prompts = Arc::get_mut(&mut self.prompts).unwrap().get_mut();
        prompts.insert(name, (prompt, handler));
        
        self
    }

    /// Handle a JSON-RPC request
    pub async fn handle_request(&self, request: JsonRpcRequest) -> Result<serde_json::Value, McpError> {
        let method = request.method.as_str();
        let params = request.params.unwrap_or(serde_json::json!({}));

        match method {
            "initialize" => {
                let _params: InitializeParams = serde_json::from_value(params)
                    .map_err(|e| McpError::InvalidParams(e.to_string()))?;
                
                *self.initialized.write().await = true;
                
                let result = InitializeResult {
                    protocol_version: MCP_VERSION.into(),
                    capabilities: self.capabilities.clone(),
                    server_info: self.server_info.clone(),
                };
                
                Ok(serde_json::to_value(result)?)
            }

            "tools/list" => {
                self.check_initialized().await?;
                
                let tools = self.tools.read().await;
                let tool_list: Vec<Tool> = tools.values().map(|(t, _)| t.clone()).collect();
                
                let result = ListToolsResult {
                    tools: tool_list,
                    next_cursor: None,
                };
                
                Ok(serde_json::to_value(result)?)
            }

            "tools/call" => {
                self.check_initialized().await?;
                
                let params: CallToolParams = serde_json::from_value(params)
                    .map_err(|e| McpError::InvalidParams(e.to_string()))?;
                
                let tools = self.tools.read().await;
                let (_, handler) = tools
                    .get(&params.name)
                    .ok_or_else(|| McpError::ToolNotFound(params.name.clone()))?;
                
                let handler = handler.clone();
                drop(tools);
                
                let args = params.arguments.unwrap_or(serde_json::json!({}));
                let result = handler(args).await?;
                
                Ok(serde_json::to_value(result)?)
            }

            "resources/list" => {
                self.check_initialized().await?;
                
                let resources = self.resources.read().await;
                let resource_list: Vec<Resource> = resources.values().map(|(r, _)| r.clone()).collect();
                
                let result = ListResourcesResult {
                    resources: resource_list,
                    next_cursor: None,
                };
                
                Ok(serde_json::to_value(result)?)
            }

            "resources/read" => {
                self.check_initialized().await?;
                
                let params: ReadResourceParams = serde_json::from_value(params)
                    .map_err(|e| McpError::InvalidParams(e.to_string()))?;
                
                let resources = self.resources.read().await;
                let (_, handler) = resources
                    .get(&params.uri)
                    .ok_or_else(|| McpError::ResourceNotFound(params.uri.clone()))?;
                
                let handler = handler.clone();
                drop(resources);
                
                let result = handler(params.uri).await?;
                
                Ok(serde_json::to_value(result)?)
            }

            "prompts/list" => {
                self.check_initialized().await?;
                
                let prompts = self.prompts.read().await;
                let prompt_list: Vec<Prompt> = prompts.values().map(|(p, _)| p.clone()).collect();
                
                let result = ListPromptsResult {
                    prompts: prompt_list,
                    next_cursor: None,
                };
                
                Ok(serde_json::to_value(result)?)
            }

            "prompts/get" => {
                self.check_initialized().await?;
                
                let params: GetPromptParams = serde_json::from_value(params)
                    .map_err(|e| McpError::InvalidParams(e.to_string()))?;
                
                let prompts = self.prompts.read().await;
                let (_, handler) = prompts
                    .get(&params.name)
                    .ok_or_else(|| McpError::PromptNotFound(params.name.clone()))?;
                
                let handler = handler.clone();
                drop(prompts);
                
                let args = params.arguments.unwrap_or_default();
                let result = handler(args).await?;
                
                Ok(serde_json::to_value(result)?)
            }

            _ => Err(McpError::MethodNotFound(method.to_string())),
        }
    }

    async fn check_initialized(&self) -> Result<(), McpError> {
        let initialized = self.initialized.read().await;
        if !*initialized {
            return Err(McpError::Internal("Server not initialized".into()));
        }
        Ok(())
    }
}

// ── HTTP Server (Axum) ────────────────────────────────────────────────────────

#[cfg(feature = "server")]
pub mod http {
    use super::*;
    use axum::{
        extract::State,
        http::StatusCode,
        routing::post,
        Json, Router,
    };
    use std::net::SocketAddr;
    use std::sync::Arc;

    impl McpServer {
        /// Create an axum router for this MCP server
        pub fn router(self) -> Router {
            let server = Arc::new(self);
            Router::new()
                .route("/", post(handle_mcp_request))
                .with_state(server)
        }

        /// Start the HTTP server
        pub async fn serve(self, addr: impl Into<SocketAddr>) -> Result<(), std::io::Error> {
            let addr = addr.into();
            let app = self.router();
            let listener = tokio::net::TcpListener::bind(addr).await?;
            println!("🚀 MCP Server listening on http://{}", addr);
            axum::serve(listener, app).await
        }
    }

    async fn handle_mcp_request(
        State(server): State<Arc<McpServer>>,
        Json(request): Json<JsonRpcRequest>,
    ) -> Result<Json<JsonRpcResponse>, (StatusCode, Json<JsonRpcError>)> {
        let request_id = request.id.clone();
        
        match server.handle_request(request).await {
            Ok(result) => Ok(Json(JsonRpcResponse::new(request_id, result))),
            Err(e) => {
                let error = e.to_json_rpc_error(request_id);
                Err((StatusCode::OK, Json(error)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_server_initialize() {
        let server = McpServer::new("Test Server", "1.0.0");
        
        let request = JsonRpcRequest::new(
            RequestId::new_string("1"),
            "initialize",
            Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "Test Client",
                    "version": "1.0.0"
                }
            })),
        );
        
        let result = server.handle_request(request).await.unwrap();
        assert!(result.get("serverInfo").is_some());
    }

    #[tokio::test]
    async fn test_tool_registration() {
        let server = McpServer::new("Test", "1.0.0")
            .with_tool(
                Tool {
                    name: "test_tool".into(),
                    description: Some("A test tool".into()),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {}
                    }),
                },
                |_args| async move {
                    Ok(CallToolResult {
                        content: vec![ToolContent::text("success")],
                        is_error: None,
                    })
                },
            );

        // Initialize
        let init_req = JsonRpcRequest::new(
            RequestId::new_number(1),
            "initialize",
            Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "Test", "version": "1.0"}
            })),
        );
        server.handle_request(init_req).await.unwrap();

        // List tools
        let list_req = JsonRpcRequest::new(
            RequestId::new_number(2),
            "tools/list",
            None,
        );
        let result = server.handle_request(list_req).await.unwrap();
        let tools_result: ListToolsResult = serde_json::from_value(result).unwrap();
        assert_eq!(tools_result.tools.len(), 1);
        assert_eq!(tools_result.tools[0].name, "test_tool");

        // Call tool
        let call_req = JsonRpcRequest::new(
            RequestId::new_number(3),
            "tools/call",
            Some(serde_json::json!({
                "name": "test_tool",
                "arguments": {}
            })),
        );
        let result = server.handle_request(call_req).await.unwrap();
        let call_result: CallToolResult = serde_json::from_value(result).unwrap();
        assert_eq!(call_result.content.len(), 1);
    }
}
